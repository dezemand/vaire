//! The store and `vaire pull` — the consuming half of the registry line
//! (registry.v2.md §5–§6).
//!
//! Every test drives the real publish → pull loop over a `file://` registry, because the
//! interesting properties are all end-to-end: what a consumer ends up holding, whether it
//! can be trusted, and what happens to it on the next pull.

mod common;

use std::path::Path;

use common::Corpus;
use vaire::model::Version;
use vaire::store::Store;

// ---- fixtures ---------------------------------------------------------------------------

fn temp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// A publishable package, with its vaire home **outside its own repository** — a home
/// inside the tree would show up as an uncommitted change and `release` refuses a dirty
/// tree, correctly.
struct Publisher {
    corpus: Corpus,
    home: tempfile::TempDir,
}

impl Publisher {
    fn new(registry: &Path) -> Publisher {
        let corpus = Corpus::empty();
        std::fs::write(
            corpus.root().join("knowledge.toml"),
            "name = \"acme-glossary\"\nversion = \"1.0.0\"\ndescription = \"Shared vocabulary\"\n\
             include = [\"**/*.md\"]\ntypes = [\"term\"]\n",
        )
        .unwrap();
        corpus
            .add(
                "knowledge/torque.md",
                "---\nid: torque-vectoring\ntype: term\nname: Torque vectoring\n---\n\
                 # Torque vectoring\n\nDistributing drive torque between wheels.\n",
            )
            .commit()
            .build();
        let publisher = Publisher {
            corpus,
            home: temp(),
        };
        vaire::commands::registry::add(
            publisher.home.path(),
            "lab",
            &registry.display().to_string(),
            0,
            true,
        )
        .expect("registry add");
        publisher
    }

    fn ctx(&self) -> vaire::commands::Ctx {
        vaire::commands::Ctx::new(Some(self.corpus.root().to_path_buf()), None)
            .unwrap()
            .with_home(self.home.path().to_path_buf())
    }

    fn root(&self) -> &Path {
        self.corpus.root()
    }

    /// Add an entity and commit it, so the next release classifies as a MINOR.
    fn grow(&self, rel: &str, contents: &str) -> &Publisher {
        self.corpus.add(rel, contents).commit().build();
        self
    }

    /// Cut a release and publish it.
    fn publish(&self) -> &Publisher {
        vaire::commands::release::run(&self.ctx(), Default::default()).expect("release");
        let out = vaire::commands::push::run(
            &self.ctx(),
            vaire::commands::push::Options {
                version: None,
                registry: None,
                access: None,
                access_hint: None,
                dry_run: false,
            },
        )
        .expect("push");
        assert!(out.failed.is_empty(), "{:?}", out.failed);
        self
    }
}

/// A consumer package that depends on the glossary, with its own hermetic vaire home —
/// a machine that has never seen the glossary's working copy, which is what the store is
/// for.
struct Consumer {
    corpus: Corpus,
    home: tempfile::TempDir,
}

impl Consumer {
    fn new(registry: &Path) -> Consumer {
        let corpus = Corpus::empty();
        std::fs::write(
            corpus.root().join("knowledge.toml"),
            "name = \"acme-drivetrain\"\nversion = \"0.1.0\"\ninclude = [\"**/*.md\"]\n\
             types = [\"component\"]\n\n[dependencies]\nacme-glossary = \"^1\"\n",
        )
        .unwrap();
        corpus
            .add(
                "knowledge/diff.md",
                "---\nid: active-differential\ntype: component\nname: Active differential\n---\n\
                 # Active differential\n\nImplements [[@acme-glossary/term:torque-vectoring]].\n",
            )
            .commit();
        let consumer = Consumer {
            corpus,
            home: temp(),
        };
        vaire::commands::registry::add(
            consumer.home.path(),
            "lab",
            &registry.display().to_string(),
            0,
            true,
        )
        .expect("registry add");
        consumer
    }

    fn ctx(&self) -> vaire::commands::Ctx {
        vaire::commands::Ctx::new(Some(self.corpus.root().to_path_buf()), None)
            .unwrap()
            .with_home(self.home.path().to_path_buf())
    }

    fn store(&self) -> Store {
        Store::at(self.home.path())
    }

    fn pull(&self, spec: Option<&str>) -> vaire::output::PullOutput {
        vaire::commands::pull::run(
            &self.ctx(),
            vaire::commands::pull::Options {
                spec,
                registry: None,
                locked: false,
                dry_run: false,
            },
        )
        .expect("pull runs")
    }
}

/// Sealed entries refuse deletion, so the temp home has to be unsealed before it can go.
impl Drop for Consumer {
    fn drop(&mut self) {
        let _ = vaire::store::unseal(self.home.path());
    }
}

// ---- the round trip ---------------------------------------------------------------------

#[test]
fn a_pulled_release_becomes_a_package_the_resolver_links_like_any_other() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    let out = consumer.pull(None);
    assert!(out.failed.is_empty(), "{:?}", out.failed);
    assert_eq!(out.pulled.len(), 1, "{out:?}");
    assert!(consumer.store().has("acme-glossary", Version::new(1, 0, 0)));

    // The point of the whole design: nothing above resolution knows this came from a
    // registry rather than from a checkout.
    let indexed =
        vaire::commands::index::run(&consumer.ctx(), false, false, false, false).expect("index");
    let dep = indexed
        .dependencies
        .iter()
        .find(|d| d.name == "acme-glossary")
        .expect("the dependency is in the closure");
    assert_eq!(dep.status, "store", "{dep:?}");

    let resolved =
        vaire::commands::resolve::run(&consumer.ctx(), "@acme-glossary/term:torque-vectoring")
            .expect("a cross-package read against the store");
    assert_eq!(resolved.package.as_deref(), Some("acme-glossary"));
}

#[test]
fn the_shipped_index_is_rebuilt_rather_than_trusted() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.pull(None);

    let entry = consumer
        .store()
        .entry("acme-glossary", Version::new(1, 0, 0));
    let index = vaire::index::Index::open(&entry.join(".vaire/index.db")).expect("index opens");
    // Built here, by this vaire, from the shipped Markdown — the trust decision the whole
    // store rests on.
    assert_eq!(index.meta("materialized").unwrap().as_deref(), Some("full"));
    // Provenance is carried, though: these files really are that commit's tree, and an
    // entry that could not say which commit it is would be worth less for no gain.
    assert!(
        index.meta("last_indexed_commit").unwrap().is_some(),
        "the release's commit survives materialization"
    );
}

#[test]
fn a_store_entry_is_sealed_against_editing() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.pull(None);
    let entry = consumer
        .store()
        .entry("acme-glossary", Version::new(1, 0, 0));

    // The corpus is what "this is release 1.0.0" is a claim about, so that is what is
    // sealed.
    let corpus_file = entry.join("knowledge/torque.md");
    assert!(
        std::fs::write(&corpus_file, "tampered").is_err(),
        "a store entry's corpus must not be writable"
    );
    assert!(
        entry
            .join(".vaire/source.toml")
            .metadata()
            .unwrap()
            .permissions()
            .readonly()
    );

    // The index is deliberately *not* sealed: an embedded database is opened for writing
    // even to read it, so a sealed index would be an unreadable one rather than an
    // immutable one. Nothing rewrites it — the ensure pass skips store members by path.
    assert!(
        vaire::index::Index::open(&entry.join(".vaire/index.db")).is_ok(),
        "a sealed index would be unreadable, which is not the same as immutable"
    );
}

#[test]
fn the_ensure_pass_never_rebuilds_a_store_entry() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.pull(None);
    let index_db = consumer
        .store()
        .entry("acme-glossary", Version::new(1, 0, 0))
        .join(".vaire/index.db");

    let before = std::fs::metadata(&index_db).unwrap().modified().unwrap();
    for _ in 0..2 {
        vaire::commands::index::run(&consumer.ctx(), false, false, false, false).expect("index");
    }
    let after = std::fs::metadata(&index_db).unwrap().modified().unwrap();
    assert_eq!(before, after, "the ensure pass rewrote a sealed entry");
}

// ---- retention --------------------------------------------------------------------------

#[test]
fn a_newer_release_replaces_its_predecessor_in_the_same_major_line() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.pull(Some("acme-glossary@1.0.0"));
    assert!(consumer.store().has("acme-glossary", Version::new(1, 0, 0)));

    // A minor release: new entity, same major line.
    publisher
        .grow(
            "knowledge/yaw.md",
            "---\nid: yaw-rate\ntype: term\nname: Yaw rate\n---\n# Yaw rate\n\nRotation rate.\n",
        )
        .publish();

    let out = consumer.pull(Some("acme-glossary"));
    assert_eq!(out.pulled[0].version, "1.1.0", "{out:?}");
    // Safe because within-major substitutability is the protocol's own promise, and free
    // because the registry keeps every version forever.
    assert_eq!(out.pulled[0].replaced, ["1.0.0"]);
    assert_eq!(
        consumer.store().versions("acme-glossary"),
        [Version::new(1, 1, 0)]
    );
}

#[test]
fn pulling_a_name_asks_the_registry_rather_than_stopping_at_what_is_cached() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();
    let consumer = Consumer::new(registry.path());
    consumer.pull(Some("acme-glossary@1.0.0"));

    publisher
        .grow(
            "knowledge/yaw.md",
            "---\nid: yaw-rate\ntype: term\nname: Yaw rate\n---\n# Yaw rate\n\nRotation rate.\n",
        )
        .publish();

    // `vaire pull <name>` means "bring me the current one". Answering from the cache would
    // make the command unable to do the thing retention exists to support.
    let out = consumer.pull(Some("acme-glossary"));
    assert_eq!(out.pulled.len(), 1, "{out:?}");
    assert!(out.already.is_empty());

    // And once it is current, the same command is a clean no-op.
    let again = consumer.pull(Some("acme-glossary"));
    assert!(again.pulled.is_empty());
    assert_eq!(again.already.len(), 1);
}

// ---- resolution -------------------------------------------------------------------------

#[test]
fn an_unsatisfiable_dependency_names_the_pull_that_would_fix_it_and_fetches_nothing() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    // Resolution never reaches the network (§6), so indexing before a pull must report
    // rather than acquire.
    let indexed =
        vaire::commands::index::run(&consumer.ctx(), false, false, false, false).expect("index");
    let dep = indexed
        .dependencies
        .iter()
        .find(|d| d.name == "acme-glossary")
        .expect("declared");
    assert_eq!(dep.status, "missing");
    let note = dep.note.as_deref().unwrap_or_default();
    assert!(note.contains("vaire pull acme-glossary@^1"), "{note}");
    assert!(
        !consumer.store().has("acme-glossary", Version::new(1, 0, 0)),
        "indexing must never fetch"
    );
}

#[test]
fn a_working_copy_outranks_a_pulled_release_of_the_same_name() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.pull(None);
    // Now catalog the publisher's *working copy* on the consumer's machine — the two-worlds
    // situation. Authoring wins: a checkout is what you are editing, and resolving to a
    // published copy of it would quietly answer against yesterday.
    vaire::commands::catalog::add(consumer.home.path(), Some(publisher.root()))
        .expect("catalog add");
    std::fs::remove_dir_all(consumer.corpus.packages_dir()).ok();

    vaire::commands::index::run(&consumer.ctx(), false, false, false, false).expect("index");
    let link = consumer.corpus.packages_dir().join("acme-glossary");
    let target = std::fs::canonicalize(&link).expect("linked");
    assert_eq!(
        target,
        std::fs::canonicalize(publisher.root()).unwrap(),
        "the working copy should win over the store entry"
    );
}

// ---- trust ------------------------------------------------------------------------------

#[test]
fn an_artifact_whose_bytes_do_not_match_is_never_unpacked() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    // Corrupted (or substituted — indistinguishable from here) after publication.
    let artifact = registry
        .path()
        .join("v1/artifacts/acme-glossary/acme-glossary-1.0.0.tgz");
    std::fs::write(&artifact, b"not what was published").unwrap();

    let consumer = Consumer::new(registry.path());
    let out = consumer.pull(Some("acme-glossary"));
    assert_eq!(out.failed.len(), 1, "{out:?}");
    assert!(
        out.failed[0].reason.contains("checksum"),
        "{:?}",
        out.failed
    );
    assert!(
        !consumer
            .store()
            .entry("acme-glossary", Version::new(1, 0, 0))
            .exists(),
        "nothing was written"
    );
}

#[test]
fn an_artifact_that_climbs_out_of_its_own_directory_is_refused() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();
    let consumer = Consumer::new(registry.path());

    // Hand-built: a well-formed artifact plus one entry that escapes. The registry serves
    // whatever it is given, so containment has to hold at the consumer.
    let hostile = temp();
    let path = hostile.path().join("hostile.tgz");
    write_archive(
        &path,
        &[
            (
                "acme-glossary-1.0.0/knowledge.toml",
                "name = \"acme-glossary\"\nversion = \"1.0.0\"\n",
            ),
            ("acme-glossary-1.0.0/../../escaped.md", "# nope\n"),
        ],
    );
    republish(registry.path(), "acme-glossary", "1.0.0", &path);

    let out = consumer.pull(Some("acme-glossary@1.0.0"));
    assert_eq!(out.failed.len(), 1, "{out:?}");
    assert!(
        out.failed[0].reason.contains("safe artifact"),
        "{:?}",
        out.failed
    );
    assert!(
        !hostile.path().join("escaped.md").exists()
            && !consumer.home.path().join("escaped.md").exists()
            && !consumer.home.path().join("store/escaped.md").exists()
    );
}

#[test]
fn an_artifact_may_not_ship_anything_but_an_index_under_the_derived_directory() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();
    let consumer = Consumer::new(registry.path());

    let hostile = temp();
    let path = hostile.path().join("hostile.tgz");
    write_archive(
        &path,
        &[
            (
                "acme-glossary-1.0.0/knowledge.toml",
                "name = \"acme-glossary\"\nversion = \"1.0.0\"\n",
            ),
            // `.vaire/` is derived state this machine owns; an artifact writing into it
            // would be handing a consumer's tool its own configuration.
            (
                "acme-glossary-1.0.0/.vaire/source.toml",
                "name = \"lies\"\n",
            ),
        ],
    );
    republish(registry.path(), "acme-glossary", "1.0.0", &path);

    let out = consumer.pull(Some("acme-glossary@1.0.0"));
    assert_eq!(out.failed.len(), 1, "{out:?}");
    assert!(out.failed[0].reason.contains(".vaire"), "{:?}", out.failed);
}

#[test]
fn an_artifact_that_declares_a_different_name_than_it_was_served_under_is_refused() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();
    let consumer = Consumer::new(registry.path());

    let hostile = temp();
    let path = hostile.path().join("hostile.tgz");
    write_archive(
        &path,
        &[(
            "acme-glossary-1.0.0/knowledge.toml",
            "name = \"something-else\"\nversion = \"1.0.0\"\n",
        )],
    );
    republish(registry.path(), "acme-glossary", "1.0.0", &path);

    // Installing it under either name would make a package resolvable by a name it does
    // not claim, and which of the two is right is not answerable from here.
    let out = consumer.pull(Some("acme-glossary@1.0.0"));
    assert_eq!(out.failed.len(), 1, "{out:?}");
    assert!(
        out.failed[0].reason.contains("declares name"),
        "{:?}",
        out.failed
    );
}

// ---- helpers ----------------------------------------------------------------------------

/// Write a gzipped tar with the given `(path, contents)` entries, **verbatim** — including
/// paths a well-behaved packer would never produce.
///
/// The name goes into the header as raw bytes rather than through `set_path`, which refuses
/// `..` — sensibly, since it is a writer. A hostile archive is not written by this crate's
/// packer, so testing the reader's containment means producing one the writer would not.
fn write_archive(path: &Path, entries: &[(&str, &str)]) {
    let file = std::fs::File::create(path).unwrap();
    let gz = flate2::GzBuilder::new()
        .mtime(0)
        .write(file, flate2::Compression::new(6));
    let mut tar = tar::Builder::new(gz);
    for (name, contents) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_entry_type(tar::EntryType::Regular);
        let raw = &mut header.as_gnu_mut().expect("gnu header").name;
        let bytes = name.as_bytes();
        raw[..bytes.len()].copy_from_slice(bytes);
        header.set_cksum();
        tar.append(&header, contents.as_bytes()).unwrap();
    }
    tar.into_inner()
        .unwrap()
        .finish()
        .unwrap()
        .sync_all()
        .unwrap();
}

/// Replace a published artifact and its recorded digest, so the registry serves `artifact`
/// as if it had always been that release.
fn republish(registry: &Path, name: &str, version: &str, artifact: &Path) {
    let bytes = std::fs::read(artifact).unwrap();
    let digest = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(&bytes))
    };
    std::fs::write(
        registry.join(format!("v1/artifacts/{name}/{name}-{version}.tgz")),
        &bytes,
    )
    .unwrap();
    let index_path = registry.join(format!("v1/index/{name}.json"));
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&index_path).unwrap()).unwrap();
    for release in index["releases"].as_array_mut().unwrap() {
        if release["version"] == version {
            release["sha256"] = serde_json::Value::String(digest.clone());
            release["size"] = serde_json::Value::from(bytes.len());
        }
    }
    std::fs::write(&index_path, serde_json::to_vec_pretty(&index).unwrap()).unwrap();
}
