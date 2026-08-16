//! `knowledge.lock` and `--frozen` — reproducibility (registry.v2.md §6–§7).
//!
//! The store made an answer *obtainable*; these make it **checkable**. The lockfile records
//! what resolution chose and whether that choice can be obtained again; `--frozen` refuses
//! the answers that cannot. Together they turn "answered against acme-core 1.4.2" from a
//! description into a claim.
//!
//! The fixtures are the store suite's, because the questions are only interesting against a
//! real publish → pull loop.

mod common;

use std::path::Path;

use common::Corpus;
use vaire::lockfile::{Lockfile, Source};
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
        self.try_pull(spec, false).expect("pull runs")
    }

    fn try_pull(
        &self,
        spec: Option<&str>,
        locked: bool,
    ) -> vaire::Result<vaire::output::PullOutput> {
        vaire::commands::pull::run(
            &self.ctx(),
            vaire::commands::pull::Options {
                spec,
                registry: None,
                locked,
                dry_run: false,
            },
        )
    }

    /// A context that answers only from the store.
    fn frozen(&self) -> vaire::commands::Ctx {
        self.ctx().with_frozen(true)
    }

    fn lockfile(&self) -> Option<Lockfile> {
        Lockfile::load(self.corpus.root()).expect("lockfile reads")
    }

    /// Catalog the publisher's working copy on this machine — the two-worlds situation.
    fn also_knows_the_working_copy(&self, publisher: &Publisher) -> &Consumer {
        vaire::commands::catalog::add(self.home.path(), Some(publisher.root()))
            .expect("catalog add");
        self
    }

    fn index(&self, ctx: &vaire::commands::Ctx) -> vaire::output::IndexRunOutput {
        vaire::commands::index::run(ctx, false, false, false, false).expect("index")
    }
}

/// Sealed entries refuse deletion, so the temp home has to be unsealed before it can go.
impl Drop for Consumer {
    fn drop(&mut self) {
        let _ = vaire::store::unseal(self.home.path());
    }
}

// ---- what the lock records ---------------------------------------------------------------

#[test]
fn a_store_resolved_dependency_is_recorded_with_the_digest_that_reproduces_it() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.pull(None);
    consumer.index(&consumer.ctx());

    let mut lockfile = consumer.lockfile().expect("written");
    let entry = lockfile.packages.remove(0);
    assert_eq!(entry.name, "acme-glossary");
    assert_eq!(entry.source, Source::Registry);
    assert_eq!(entry.registry.as_deref(), Some("lab"));
    assert!(entry.reproducible(), "{entry:?}");
    // The artifact's digest, so it can be checked against what a registry serves later —
    // which is the only place a published version quietly changing would be caught.
    assert_eq!(
        entry.sha256,
        Some(
            consumer
                .store()
                .source("acme-glossary", Version::new(1, 0, 0))
                .unwrap()
                .artifact_sha256
        )
    );
}

#[test]
fn a_working_copy_is_recorded_without_a_checksum_because_it_has_none() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.also_knows_the_working_copy(&publisher);
    consumer.index(&consumer.ctx());

    let mut lockfile = consumer.lockfile().expect("written");
    let entry = lockfile.packages.remove(0);
    // Not a gap in the record — the record of a gap. A checkout has no artifact to
    // checksum and can change between two runs, so a digest here would be a
    // reproducibility claim the tool cannot keep.
    assert_eq!(entry.source, Source::Workspace);
    assert!(entry.sha256.is_none());
    assert!(!entry.reproducible());
}

// ---- reproducing ------------------------------------------------------------------------

#[test]
fn locked_reproduces_the_recorded_versions_rather_than_the_newest() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.pull(None);
    assert_eq!(
        consumer.lockfile().unwrap().packages[0].version,
        Version::new(1, 0, 0)
    );

    // The publisher moves on, and the store is cleared as if this were another machine.
    publisher
        .grow(
            "knowledge/yaw.md",
            "---\nid: yaw-rate\ntype: term\nname: Yaw rate\n---\n# Yaw rate\n\nRotation rate.\n",
        )
        .publish();
    vaire::store::unseal(consumer.home.path()).unwrap();
    std::fs::remove_dir_all(consumer.store().root()).unwrap();

    let out = consumer.try_pull(None, true).expect("locked pull");
    assert!(out.failed.is_empty(), "{:?}", out.failed);
    // 1.1.0 exists and satisfies `^1`; reproduction means what was recorded, not what is
    // current, or it would not be reproduction.
    assert_eq!(out.pulled[0].version, "1.0.0", "{out:?}");
    assert!(consumer.store().has("acme-glossary", Version::new(1, 0, 0)));
}

#[test]
fn locked_catches_a_registry_serving_different_bytes_under_a_recorded_version() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.pull(None);

    // The recorded digest no longer matches what the registry serves. `fetch` cannot catch
    // this on its own — the artifact agrees with the registry's *current* claim — so the
    // lockfile is the only thing that can.
    let mut lockfile = consumer.lockfile().unwrap();
    lockfile.packages[0].sha256 = Some("0".repeat(64));
    lockfile.write(consumer.corpus.root()).unwrap();
    vaire::store::unseal(consumer.home.path()).unwrap();
    std::fs::remove_dir_all(consumer.store().root()).unwrap();

    let out = consumer.try_pull(None, true).expect("locked pull runs");
    assert_eq!(out.failed.len(), 1, "{out:?}");
    assert!(
        out.failed[0]
            .reason
            .contains("does not match knowledge.lock"),
        "{:?}",
        out.failed
    );
    assert!(
        !consumer.store().has("acme-glossary", Version::new(1, 0, 0)),
        "nothing was materialized"
    );
}

#[test]
fn locked_refuses_a_resolution_that_cannot_be_reproduced() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.also_knows_the_working_copy(&publisher);
    consumer.index(&consumer.ctx());

    // The lock says this answer came from a checkout. Reproducing it is not possible, and
    // quietly skipping it would let a pipeline report a reproduction it did not perform.
    let Err(e) = consumer.try_pull(None, true) else {
        panic!("a workspace-resolved entry cannot be reproduced");
    };
    assert!(e.to_string().contains("cannot be obtained again"), "{e}");
}

#[test]
fn locked_with_no_lockfile_says_how_to_get_one() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();
    let consumer = Consumer::new(registry.path());

    let Err(e) = consumer.try_pull(None, true) else {
        panic!("there is nothing to reproduce");
    };
    assert!(e.to_string().contains("run `vaire pull` once"), "{e}");
}

// ---- --frozen ---------------------------------------------------------------------------

#[test]
fn frozen_refuses_an_answer_that_came_from_a_working_copy() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.also_knows_the_working_copy(&publisher);
    consumer.index(&consumer.ctx());

    // Ordinarily this is exactly right: authoring wants the working copy.
    assert!(
        vaire::commands::resolve::run(&consumer.ctx(), "@acme-glossary/term:torque-vectoring")
            .is_ok()
    );

    // Under `--frozen` it is refused, because the version it answered from is not one
    // anybody else can obtain — which is the whole thing the flag exists to guarantee.
    let Err(e) =
        vaire::commands::resolve::run(&consumer.frozen(), "@acme-glossary/term:torque-vectoring")
    else {
        panic!("a working copy is not a reproducible answer");
    };
    let message = e.to_string();
    assert!(
        message.contains("--frozen answers only from the store"),
        "{message}"
    );
    // And it says what to do about it rather than only that it will not.
    assert!(message.contains("vaire pull acme-glossary"), "{message}");
}

#[test]
fn frozen_answers_from_the_store() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.pull(None);
    consumer.index(&consumer.frozen());

    let resolved =
        vaire::commands::resolve::run(&consumer.frozen(), "@acme-glossary/term:torque-vectoring")
            .expect("a store entry is a reproducible answer");
    assert_eq!(resolved.package.as_deref(), Some("acme-glossary"));
}

#[test]
fn frozen_links_from_the_store_without_consulting_the_catalog() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    // Both worlds are available. Ordinary resolution prefers the working copy; frozen
    // resolution must not, and must not need the catalog to find that out.
    consumer.also_knows_the_working_copy(&publisher);
    consumer.pull(None);

    let indexed = consumer.index(&consumer.frozen());
    let dep = indexed
        .dependencies
        .iter()
        .find(|d| d.name == "acme-glossary")
        .expect("declared");
    assert_eq!(dep.status, "store", "{dep:?}");
}

/// A second publishable package, so the glossary can depend on something and the consumer
/// can acquire a **transitive** member — the case that separates "the closure" from "the
/// direct dependencies".
fn units(registry: &Path) -> Publisher {
    let corpus = Corpus::empty();
    std::fs::write(
        corpus.root().join("knowledge.toml"),
        "name = \"acme-units\"\nversion = \"1.0.0\"\ninclude = [\"**/*.md\"]\ntypes = [\"unit\"]\n",
    )
    .unwrap();
    corpus
        .add(
            "knowledge/newton-metre.md",
            "---\nid: newton-metre\ntype: unit\nname: Newton metre\n---\n# Newton metre\n\nTorque.\n",
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

#[test]
fn pulling_does_not_prune_the_transitive_closure_that_indexing_recorded() {
    let registry = temp();
    let units = units(registry.path());
    units.publish();

    // The glossary depends on the units package, so a consumer of the glossary acquires
    // `acme-units` transitively — it is in the closure and *not* in the consumer's manifest.
    let publisher = Publisher::new(registry.path());
    std::fs::write(
        publisher.root().join("knowledge.toml"),
        "name = \"acme-glossary\"\nversion = \"1.0.0\"\ninclude = [\"**/*.md\"]\n\
         types = [\"term\"]\n\n[dependencies]\nacme-units = \"^1\"\n",
    )
    .unwrap();
    vaire::commands::catalog::add(publisher.home.path(), Some(units.root())).expect("catalog add");
    publisher.corpus.commit().build();
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.pull(Some("acme-glossary"));
    consumer.pull(Some("acme-units"));
    consumer.index(&consumer.ctx());
    assert!(
        consumer.lockfile().unwrap().get("acme-units").is_some(),
        "indexing records the whole closure"
    );

    // `merged` forgets whatever is absent from the list it is given, so a pull passing only
    // the *direct* dependencies would delete this — and a later `--locked` would then
    // reproduce half a closure while claiming to be exact (cli.md §4.13).
    consumer.pull(Some("acme-glossary"));
    let after = consumer.lockfile().expect("still written");
    assert!(
        after.get("acme-units").is_some(),
        "pull pruned a transitive entry indexing had recorded: {after:?}"
    );
}

#[test]
fn frozen_is_store_only_even_outside_a_package() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.pull(None);
    // Both worlds hold the name. Unfrozen, the working copy displaces the store entry; under
    // `--frozen` that displacement must not hide the store entry, or the package would be
    // invisible *and* the working copy refused — a release the store actually holds, unusable.
    vaire::commands::catalog::add(consumer.home.path(), Some(publisher.root()))
        .expect("catalog add");

    let frozen =
        vaire::commands::Ctx::rootless_with(consumer.home.path().to_path_buf(), None, true)
            .expect("a frozen rootless session");
    assert!(frozen.is_frozen());
    let ws = frozen.workspace().expect("the view");
    let located = ws
        .locate(&ws.current(), "acme-glossary")
        .expect("the store entry answers");
    assert!(
        consumer.store().contains(&located.root),
        "a frozen session must answer from the store, got {}",
        located.root.display()
    );
}

#[test]
fn a_named_pull_outside_a_package_leaves_the_home_lockfile_alone() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();
    let consumer = Consumer::new(registry.path());

    // A file that has nothing to do with this pull. Rootless `ctx.repo` is the vaire home
    // and a rootless config declares nothing, so a merge-and-write here would resolve to an
    // empty lockfile — and `write` removes the file when there is nothing to record.
    let home_lock = vaire::lockfile::path_for(consumer.home.path());
    std::fs::write(&home_lock, "lockfile_version = 1\n").unwrap();

    let ctx = vaire::commands::Ctx::rootless(consumer.home.path().to_path_buf()).expect("rootless");
    vaire::commands::pull::run(
        &ctx,
        vaire::commands::pull::Options {
            spec: Some("acme-glossary"),
            registry: None,
            locked: false,
            dry_run: false,
        },
    )
    .expect("a named pull works from anywhere");

    assert!(home_lock.is_file(), "the home lockfile was removed");
}

#[test]
fn a_stale_lock_is_imprecise_rather_than_wrong() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.pull(None);
    publisher
        .grow(
            "knowledge/yaw.md",
            "---\nid: yaw-rate\ntype: term\nname: Yaw rate\n---\n# Yaw rate\n\nRotation rate.\n",
        )
        .publish();

    // Advancing within the major line is silent adoption, which within-major
    // substitutability is exactly the promise that permits — so the lock catches up rather
    // than the resolution being refused.
    consumer.pull(Some("acme-glossary"));
    consumer.index(&consumer.ctx());
    assert_eq!(
        consumer.lockfile().unwrap().packages[0].version,
        Version::new(1, 1, 0)
    );
}
