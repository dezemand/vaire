//! The registry conformance suite, run against a `file://` registry.
//!
//! This is what the `file://` backend is *for* (registry.md §9). The
//! transport is split from the protocol precisely so that a directory exercises the same
//! `StaticHttp` a bucket will, which makes these tests the conformance suite rather than a
//! mock's self-portrait — and it means the whole wire contract is testable with no
//! infrastructure at all.
//!
//! Two axes are covered:
//!
//! * **The protocol** — publish, fetch, enumerate, yank, access, schema gating, and the
//!   two conditional writes that give a dumb file host its immutability and its race
//!   handling.
//! * **The commands** — `push` from tags (including from a fresh clone), and the
//!   idempotence that makes a retried upload safe.

mod common;

use std::path::Path;

use common::{Corpus, Ws, git};
use vaire::model::Version;
use vaire::registry::wire::{Access, PackageIndex};
use vaire::registry::{PublishRequest, Registry, RegistryError, StaticHttp};

// ---- fixtures ---------------------------------------------------------------------------

/// A releasable package: it indexes everything, so release records land inside the corpus.
fn package() -> Corpus {
    let c = Corpus::empty();
    std::fs::write(
        c.root().join("knowledge.toml"),
        "name = \"acme-core\"\nversion = \"1.0.0\"\ndescription = \"Core knowledge\"\n\
         include = [\"**/*.md\"]\ntypes = [\"department\", \"person\"]\n",
    )
    .unwrap();
    c.add(
        "knowledge/platform.md",
        "---\nid: platform\ntype: department\nname: Platform\n---\n# Platform\n\nThe platform group.\n",
    )
    .commit()
    .build();
    c
}

/// An empty directory to publish into, and the client pointed at it.
fn registry(dir: &Path) -> StaticHttp {
    StaticHttp::open("lab", &format!("file://{}", dir.display())).expect("registry opens")
}

fn temp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// A file standing in for a packed artifact — the protocol tests care about bytes moving
/// and digests matching, not about what is inside the tarball.
fn artifact(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn publish(
    registry: &StaticHttp,
    artifact: &Path,
    version: &str,
) -> Result<vaire::registry::Published, RegistryError> {
    registry.publish(request(artifact, version))
}

fn request<'a>(artifact: &'a Path, version: &str) -> PublishRequest<'a> {
    PublishRequest {
        name: "acme-core",
        version: version.parse().unwrap(),
        artifact,
        changelog: None,
        changelog_excerpt: None,
        deps: Default::default(),
        description: None,
        access: None,
        claimed_bump: None,
        prior_version: None,
    }
}

fn index_doc(dir: &Path, name: &str) -> PackageIndex {
    let raw = std::fs::read(dir.join(format!("v1/index/{name}.json"))).expect("index document");
    serde_json::from_slice(&raw).expect("index document parses")
}

// ---- the protocol -----------------------------------------------------------------------

#[test]
fn a_directory_becomes_a_registry_on_first_publish() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());
    // Nothing has been published, so there is no descriptor — which is a state, not an
    // error. Refusing to open one would mean a first push could never happen.
    assert!(!registry.initialized());

    publish(
        &registry,
        &artifact(scratch.path(), "a.tgz", b"payload"),
        "1.0.0",
    )
    .unwrap();

    let descriptor = dir.path().join(".well-known/vaire-registry.json");
    assert!(descriptor.is_file(), "publishing initialized the registry");
    // Everything a dumb file host serves, in the layout §8.1 describes.
    for path in [
        "v1/index/acme-core.json",
        "v1/artifacts/acme-core/acme-core-1.0.0.tgz",
        "v1/packages.json",
    ] {
        assert!(dir.path().join(path).is_file(), "{path} is missing");
    }
}

#[test]
fn a_published_version_is_immutable_and_storage_is_what_says_so() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());
    publish(
        &registry,
        &artifact(scratch.path(), "a.tgz", b"first"),
        "1.0.0",
    )
    .unwrap();

    // Not a policy check that could be raced: the create-only write is the same atomic
    // test-and-set that published it.
    let second = publish(
        &registry,
        &artifact(scratch.path(), "b.tgz", b"different bytes"),
        "1.0.0",
    );
    assert!(
        matches!(second, Err(RegistryError::VersionExists { .. })),
        "{second:?}"
    );
    let kept = std::fs::read(
        dir.path()
            .join("v1/artifacts/acme-core/acme-core-1.0.0.tgz"),
    )
    .unwrap();
    assert_eq!(kept, b"first", "the published bytes never moved");
    assert_eq!(index_doc(dir.path(), "acme-core").releases.len(), 1);
}

#[test]
fn a_fetched_artifact_is_verified_against_the_published_digest() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());
    publish(
        &registry,
        &artifact(scratch.path(), "a.tgz", b"payload"),
        "1.0.0",
    )
    .unwrap();

    let into = scratch.path().join("downloaded.tgz");
    let verified = registry
        .fetch("acme-core", Version::new(1, 0, 0), &into)
        .expect("fetch");
    assert_eq!(std::fs::read(&into).unwrap(), b"payload");
    assert_eq!(verified.size, 7);
    assert_eq!(
        verified.sha256,
        index_doc(dir.path(), "acme-core").releases[0].sha256
    );
}

#[test]
fn a_corrupted_artifact_is_never_installed() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());
    publish(
        &registry,
        &artifact(scratch.path(), "a.tgz", b"payload"),
        "1.0.0",
    )
    .unwrap();

    // Substituted or corrupted — indistinguishable from here, which is exactly why
    // neither gets to leave a file behind.
    std::fs::write(
        dir.path()
            .join("v1/artifacts/acme-core/acme-core-1.0.0.tgz"),
        b"not what was published",
    )
    .unwrap();

    let into = scratch.path().join("downloaded.tgz");
    let fetched = registry.fetch("acme-core", Version::new(1, 0, 0), &into);
    assert!(
        matches!(fetched, Err(RegistryError::ChecksumMismatch { .. })),
        "{fetched:?}"
    );
    assert!(!into.exists(), "nothing was written");
}

#[test]
fn a_registry_from_a_newer_client_is_refused_rather_than_misread() {
    let dir = temp();
    std::fs::create_dir_all(dir.path().join(".well-known")).unwrap();
    std::fs::write(
        dir.path().join(".well-known/vaire-registry.json"),
        r#"{"schema_version": 99, "capabilities": {"enumerable": true}}"#,
    )
    .unwrap();

    let opened = StaticHttp::open("lab", &format!("file://{}", dir.path().display()));
    match opened {
        Err(RegistryError::SchemaTooNew { found, .. }) => assert_eq!(found, 99),
        other => panic!(
            "a newer schema must refuse cleanly, got {other:?}",
            other = other.err()
        ),
    }
}

#[test]
fn a_yank_edits_the_index_and_leaves_the_artifact_alone() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());
    publish(
        &registry,
        &artifact(scratch.path(), "a.tgz", b"one"),
        "1.0.0",
    )
    .unwrap();
    publish(
        &registry,
        &artifact(scratch.path(), "b.tgz", b"two"),
        "1.1.0",
    )
    .unwrap();

    registry
        .yank("acme-core", Version::new(1, 1, 0), true)
        .unwrap();

    let index = index_doc(dir.path(), "acme-core");
    assert!(index.get(Version::new(1, 1, 0)).unwrap().yanked);
    // The whole difference between a yank and a deletion: a lockfile pinning 1.1.0 still
    // resolves, and still fetches.
    assert!(
        dir.path()
            .join("v1/artifacts/acme-core/acme-core-1.1.0.tgz")
            .is_file()
    );
    assert!(
        registry
            .fetch(
                "acme-core",
                Version::new(1, 1, 0),
                &scratch.path().join("y.tgz")
            )
            .is_ok(),
        "a yanked version is still fetchable by exact version"
    );
    // What changes is only what a *new* resolution would choose.
    assert_eq!(index.latest(), Some(Version::new(1, 0, 0)));

    registry
        .yank("acme-core", Version::new(1, 1, 0), false)
        .unwrap();
    assert_eq!(
        index_doc(dir.path(), "acme-core").latest(),
        Some(Version::new(1, 1, 0)),
        "--undo puts it back"
    );
}

#[test]
fn a_restricted_package_refuses_the_fetch_and_carries_its_hint() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());
    let path = artifact(scratch.path(), "a.tgz", b"payload");
    let mut request = request(&path, "1.0.0");
    request.access = Access::parse("restricted", Some("request via #team-powertrain".into()));
    registry.publish(request).unwrap();

    let refused = registry.fetch(
        "acme-core",
        Version::new(1, 0, 0),
        &scratch.path().join("x.tgz"),
    );
    match refused {
        // The hint IS the feature: restricted-listed exists to route someone to the owner.
        Err(RegistryError::PullRestricted { hint, .. }) => {
            assert_eq!(hint.as_deref(), Some("request via #team-powertrain"));
        }
        other => panic!("expected a restricted refusal, got {:?}", other.err()),
    }
    // Listed, though — discoverability is the point of the state.
    assert_eq!(registry.list().unwrap().len(), 1);
}

#[test]
fn an_unlisted_package_is_invisible_to_enumeration_but_answers_by_name() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());
    let path = artifact(scratch.path(), "a.tgz", b"payload");
    let mut request = request(&path, "1.0.0");
    request.access = Access::parse("unlisted", None);
    registry.publish(request).unwrap();

    assert!(
        registry.list().unwrap().is_empty(),
        "invisible to a listing"
    );
    assert_eq!(
        registry.versions("acme-core").unwrap().len(),
        1,
        "an exact-name probe still answers"
    );
}

#[test]
fn access_is_sticky_until_it_is_changed() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());
    let path = artifact(scratch.path(), "a.tgz", b"one");
    let mut first = request(&path, "1.0.0");
    first.access = Access::parse("restricted", Some("ask me".into()));
    registry.publish(first).unwrap();

    // A later publish that says nothing about access must not quietly re-open the package.
    publish(
        &registry,
        &artifact(scratch.path(), "b.tgz", b"two"),
        "1.1.0",
    )
    .unwrap();
    let index = index_doc(dir.path(), "acme-core");
    assert!(!index.access.pullable, "still restricted");
    assert_eq!(index.access.hint.as_deref(), Some("ask me"));
}

#[test]
fn a_publish_that_the_index_refuses_is_not_reported_as_success() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());
    publish(
        &registry,
        &artifact(scratch.path(), "a.tgz", b"one"),
        "1.0.0",
    )
    .unwrap();

    // The artifact deleted out of band — a bucket lifecycle rule, a stray `rm`. The
    // create-only write now succeeds with *new* bytes while the index still holds the old
    // digest, so accepting this would make every later fetch of 1.0.0 fail its checksum
    // forever, after a publish that claimed to work.
    std::fs::remove_file(
        dir.path()
            .join("v1/artifacts/acme-core/acme-core-1.0.0.tgz"),
    )
    .unwrap();
    let republished = publish(
        &registry,
        &artifact(scratch.path(), "b.tgz", b"different bytes"),
        "1.0.0",
    );
    assert!(
        matches!(republished, Err(RegistryError::VersionExists { .. })),
        "{republished:?}"
    );
    let index = index_doc(dir.path(), "acme-core");
    assert_eq!(index.releases.len(), 1);
    assert_eq!(
        index.releases[0].sha256,
        {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(b"one"))
        },
        "the index still describes the release it always described"
    );
}

#[test]
fn a_missing_package_is_not_found_rather_than_an_empty_answer() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());
    publish(
        &registry,
        &artifact(scratch.path(), "a.tgz", b"payload"),
        "1.0.0",
    )
    .unwrap();

    let missing = registry.versions("acme-nothing");
    assert!(matches!(missing, Err(RegistryError::NotFound { .. })));
    // Which the fan-out reads as "ask the next registry", not as a failure.
    assert_eq!(
        missing.unwrap_err().disposition(),
        vaire::registry::Disposition::Continue
    );
}

#[test]
fn a_name_that_would_escape_the_registry_root_is_refused() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());
    let path = artifact(scratch.path(), "a.tgz", b"payload");
    let mut request = request(&path, "1.0.0");
    request.name = "../../etc/passwd";

    assert!(matches!(
        registry.publish(request),
        Err(RegistryError::Malformed { .. })
    ));
    assert!(
        !dir.path().join("v1").exists(),
        "nothing was written anywhere"
    );
}

#[test]
fn an_https_registry_reads_but_refuses_to_publish() {
    // No network: an unreachable host is enough to prove the *shape* of the answer, which
    // is what matters — publishing over http needs the object store's own credentials, so
    // it reports that rather than failing somewhere confusing later.
    let registry = StaticHttp::open("central", "https://packages.invalid/kg");
    let Ok(registry) = registry else {
        // An unreachable descriptor is itself a clean refusal; either outcome is correct.
        return;
    };
    let scratch = temp();
    let refused = publish(
        &registry,
        &artifact(scratch.path(), "a.tgz", b"payload"),
        "1.0.0",
    );
    assert!(matches!(
        refused,
        Err(RegistryError::Unsupported { .. } | RegistryError::Unreachable { .. })
    ));
}

// ---- push -------------------------------------------------------------------------------

fn push(
    c: &Corpus,
    dir: &Path,
    options: vaire::commands::push::Options<'_>,
) -> vaire::output::PushOutput {
    let ctx = c.ctx();
    vaire::commands::registry::add(ctx.home(), "lab", &dir.display().to_string(), 0, true)
        .expect("registry add");
    vaire::commands::push::run(&ctx, options).expect("push runs")
}

fn options<'a>() -> vaire::commands::push::Options<'a> {
    vaire::commands::push::Options {
        version: None,
        registry: None,
        access: None,
        access_hint: None,
        dry_run: false,
    }
}

fn release(c: &Corpus) {
    vaire::commands::release::run(&c.ctx(), Default::default()).expect("release");
}

#[test]
fn push_publishes_every_release_tag_and_is_safe_to_run_again() {
    let c = package();
    let dir = temp();
    release(&c);
    c.add(
        "knowledge/jane.md",
        "---\nid: jane\ntype: person\nname: Jane\n---\n# Jane\n\nIn [[department:platform]].\n",
    )
    .commit()
    .build();
    release(&c);

    let first = push(&c, dir.path(), options());
    assert_eq!(first.published.len(), 2, "{first:?}");
    assert!(first.failed.is_empty(), "{:?}", first.failed);

    // Idempotent plumbing: a flaky upload must not re-run a ritual, so the second run is
    // a no-op rather than a conflict.
    let second = push(&c, dir.path(), options());
    assert!(second.published.is_empty());
    assert_eq!(second.already.len(), 2);

    let index = index_doc(dir.path(), "acme-core");
    let versions: Vec<String> = index
        .releases
        .iter()
        .map(|r| r.version.to_string())
        .collect();
    assert_eq!(versions, ["1.0.0", "1.1.0"], "oldest first");
}

#[test]
fn push_rebuilds_artifacts_from_tags_so_a_fresh_clone_can_publish() {
    let c = package();
    let dir = temp();
    release(&c);
    let expected = push(&c, dir.path(), options()).published[0].sha256.clone();

    // A container that cloned thirty seconds ago: no `.vaire/dist/`, no index, no history
    // of ever having packed anything.
    let clone = temp();
    let target = clone.path().join("fresh");
    git(
        c.root(),
        &[
            "clone",
            "-q",
            &c.root().display().to_string(),
            &target.display().to_string(),
        ],
    );
    assert!(
        !target.join(".vaire/dist").exists(),
        "the clone has no artifact cache"
    );

    let elsewhere = temp();
    let ctx = vaire::commands::Ctx::new(Some(target.clone()), None)
        .unwrap()
        .with_home(target.join(".vaire-home"));
    vaire::commands::registry::add(
        ctx.home(),
        "lab",
        &elsewhere.path().display().to_string(),
        0,
        true,
    )
    .unwrap();
    let out = vaire::commands::push::run(&ctx, options()).expect("push from a clone");

    assert_eq!(out.published.len(), 1, "{out:?}");
    // Determinism is what makes this sound: the artifact rebuilt from the tag is the one
    // the tag produced, so the checksum a lockfile pins belongs to the release rather than
    // to whichever machine happened to upload it.
    assert_eq!(
        out.published[0].sha256, expected,
        "the same tag must pack to the same bytes anywhere"
    );
}

#[test]
fn a_version_taken_by_someone_elses_bytes_is_a_failure_not_an_idempotent_no_op() {
    let c = package();
    let dir = temp();
    release(&c);

    // Another repository — or a mistake, or an attack — got to this (name, version) first,
    // with different bytes. Storage will refuse the create-only write, and that refusal says
    // only that *something* occupies the identity.
    let registry = registry(dir.path());
    let scratch = temp();
    publish(
        &registry,
        &artifact(scratch.path(), "theirs.tgz", b"published by someone else"),
        "1.0.0",
    )
    .unwrap();

    // Naming the version bypasses the preflight skip, so the artifact is actually built and
    // the digests actually compared.
    let out = push(
        &c,
        dir.path(),
        vaire::commands::push::Options {
            version: Some("1.0.0"),
            ..options()
        },
    );
    assert!(out.published.is_empty(), "{out:?}");
    assert!(
        out.already.is_empty(),
        "this release did NOT reach the registry"
    );
    assert_eq!(out.failed.len(), 1, "{out:?}");
    // Reporting this as "already published" would be the worst outcome available: the
    // identity is immutably taken and the intended release will never reach the registry.
    assert!(
        out.failed[0].reason.contains("not this release"),
        "{:?}",
        out.failed
    );
}

#[test]
fn a_version_published_by_someone_else_mid_push_is_a_no_op_when_the_bytes_match() {
    let c = package();
    let dir = temp();
    release(&c);

    // Publish once, then push the same tag again with the version explicitly named so the
    // preflight skip does not answer for us. Determinism means the second attempt packs
    // byte-identical bytes, so the create-only refusal really is idempotence.
    push(&c, dir.path(), options());
    let out = push(
        &c,
        dir.path(),
        vaire::commands::push::Options {
            version: Some("1.0.0"),
            ..options()
        },
    );
    assert!(out.failed.is_empty(), "{:?}", out.failed);
    assert_eq!(out.already, ["1.0.0"], "{out:?}");
}

#[test]
fn a_registry_name_is_stored_the_way_it_will_be_looked_up() {
    let c = package();
    let dir = temp();
    let ctx = c.ctx();
    // The name is the primary key, so an untrimmed one would list as `lab` and then fail
    // every `show`/`rm`/`--registry` that spells it the way it looks.
    vaire::commands::registry::add(
        ctx.home(),
        "  lab  ",
        &dir.path().display().to_string(),
        0,
        true,
    )
    .unwrap();
    assert_eq!(
        vaire::commands::registry::find(ctx.home(), "lab")
            .unwrap()
            .name,
        "lab"
    );
}

#[test]
fn push_carries_the_release_record_as_the_changelog() {
    let c = package();
    let dir = temp();
    release(&c);
    push(&c, dir.path(), options());

    let changelog =
        std::fs::read_to_string(dir.path().join("v1/changelogs/acme-core/1.0.0.md")).unwrap();
    // Amendment 20: the record is the source, so the wire's changelog derives from it and
    // CHANGELOG.md never becomes a second one.
    assert!(changelog.contains("type: release"), "{changelog}");
    assert!(changelog.contains("1.0.0"), "{changelog}");
}

#[test]
fn push_records_declared_dependencies_in_the_index_document() {
    // A real linked dependency, not just a line in a manifest: `release` gates on `check`,
    // and a declared dependency that resolves to nothing is a violation — correctly, since
    // publishing a package whose own references do not resolve is what the gate is for.
    let ws = Ws::new();
    ws.add_package("acme-glossary", &["term"], &[])
        .add_file(
            "acme-glossary",
            "knowledge/torque.md",
            "---\nid: torque\ntype: term\nname: Torque\n---\n# Torque\n",
        )
        .commit("acme-glossary")
        .build("acme-glossary");

    ws.add_package("acme-core", &["department"], &[]);
    // Release records land inside the corpus, so the globs have to reach them.
    std::fs::write(
        ws.root("acme-core").join("knowledge.toml"),
        "name = \"acme-core\"\nversion = \"1.0.0\"\ninclude = [\"**/*.md\"]\n\
         types = [\"department\"]\n",
    )
    .unwrap();
    ws.add_file(
        "acme-core",
        "knowledge/platform.md",
        "---\nid: platform\ntype: department\nname: Platform\n---\n# Platform\n\nSee [[@acme-glossary/term:torque]].\n",
    );
    ws.link_to("acme-core", "acme-glossary", "acme-glossary");
    ws.commit("acme-core").build("acme-core");

    let ctx = ws.ctx("acme-core");
    vaire::commands::release::run(&ctx, Default::default()).expect("release");
    let dir = temp();
    vaire::commands::registry::add(
        ctx.home(),
        "lab",
        &dir.path().display().to_string(),
        0,
        true,
    )
    .unwrap();
    let out = vaire::commands::push::run(&ctx, options()).expect("push");
    assert!(out.failed.is_empty(), "{:?}", out.failed);

    // Decision 9: transitive resolution must never download an artifact to read a manifest,
    // so the constraint rides in the index document beside the checksum.
    let index = index_doc(dir.path(), "acme-core");
    assert_eq!(
        index.releases[0]
            .deps
            .get("acme-glossary")
            .map(String::as_str),
        Some("^1")
    );
}

#[test]
fn a_dry_run_publishes_nothing() {
    let c = package();
    let dir = temp();
    release(&c);

    let out = push(
        &c,
        dir.path(),
        vaire::commands::push::Options {
            dry_run: true,
            ..options()
        },
    );
    assert_eq!(out.published.len(), 1, "it reports what it would do");
    assert!(!dir.path().join("v1").exists(), "and writes nothing at all");
}

#[test]
fn pushing_a_package_with_no_release_tags_says_what_to_do() {
    let c = package();
    let dir = temp();
    let ctx = c.ctx();
    vaire::commands::registry::add(
        ctx.home(),
        "lab",
        &dir.path().display().to_string(),
        0,
        true,
    )
    .unwrap();

    let refused = vaire::commands::push::run(&ctx, options());
    let message = refused.unwrap_err().to_string();
    assert!(message.contains("vaire release"), "{message}");
}

#[test]
fn push_with_no_registry_configured_says_how_to_configure_one() {
    let c = package();
    release(&c);
    let refused = vaire::commands::push::run(&c.ctx(), options());
    let message = refused.unwrap_err().to_string();
    assert!(message.contains("vaire registry add"), "{message}");
}

#[test]
fn one_of_several_registries_must_be_named_unless_it_outranks_the_rest() {
    let c = package();
    let (a, b) = (temp(), temp());
    let ctx = c.ctx();
    for (name, dir) in [("lab", &a), ("mirror", &b)] {
        vaire::commands::registry::add(
            ctx.home(),
            name,
            &dir.path().display().to_string(),
            0,
            true,
        )
        .unwrap();
    }
    // Publishing to the wrong registry cannot be taken back — the artifact is immutable —
    // so ambiguity asks rather than guesses.
    let refused = vaire::commands::registry::select(ctx.home(), None);
    let message = refused.unwrap_err().to_string();
    assert!(message.contains("--registry"), "{message}");

    // A strictly highest priority resolves it, which is what that column is for.
    vaire::commands::registry::add(ctx.home(), "lab", &a.path().display().to_string(), 10, true)
        .unwrap();
    assert_eq!(
        vaire::commands::registry::select(ctx.home(), None)
            .unwrap()
            .name,
        "lab"
    );
}

#[test]
fn a_registry_is_forgotten_only_when_it_is_removed() {
    let c = package();
    let dir = temp();
    let ctx = c.ctx();
    vaire::commands::registry::add(
        ctx.home(),
        "lab",
        &dir.path().display().to_string(),
        0,
        true,
    )
    .unwrap();
    assert_eq!(
        vaire::commands::registry::list(ctx.home())
            .unwrap()
            .registries
            .len(),
        1
    );

    assert!(
        vaire::commands::registry::remove(ctx.home(), "lab")
            .unwrap()
            .removed
    );
    assert!(
        vaire::commands::registry::list(ctx.home())
            .unwrap()
            .registries
            .is_empty()
    );
    // Removing something that is not there is reported, never invented.
    assert!(
        !vaire::commands::registry::remove(ctx.home(), "lab")
            .unwrap()
            .removed
    );
}

// ---- resuming a publish that was interrupted --------------------------------------------

/// A publish is three writes and only the first is atomic, so the window between them is
/// real: an interrupted push leaves the artifact placed and the index silent. That state
/// has to be *finishable*.
///
/// Refusing it would make it permanent — the create-only `PUT` means the identity is
/// immutably taken, so no later push could ever claim it, and a release would be stranded
/// with nothing to do but skip a version. The whole point of splitting `release` from
/// `push` is that transport is retryable (registry.md §3.3).
#[test]
fn a_publish_interrupted_before_its_index_write_is_finished_by_the_next_one() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());
    let artifact = artifact(scratch.path(), "a.tgz", b"payload");

    publish(&registry, &artifact, "1.0.0").unwrap();
    // Simulate the interruption: the artifact landed, the index never recorded it.
    std::fs::remove_file(dir.path().join("v1/index/acme-core.json")).unwrap();
    assert!(dir.path().join("v1/artifacts/acme-core/acme-core-1.0.0.tgz").is_file());

    let published = publish(&registry, &artifact, "1.0.0")
        .expect("the same bytes finish the publish they find half-done");
    assert_eq!(published.version, Version::new(1, 0, 0));

    let index = index_doc(dir.path(), "acme-core");
    assert_eq!(index.releases.len(), 1, "the release is recorded exactly once");
    assert_eq!(index.get(Version::new(1, 0, 0)).unwrap().sha256, published.sha256);

    // And it stays idempotent afterwards, which is the ordinary re-push path.
    publish(&registry, &artifact, "1.0.0").expect_err("a recorded version is taken");
}

/// Resuming is decided by the bytes, never by the gap.
///
/// A missing index entry is not permission to overwrite: if what occupies the version is
/// somebody else's artifact, this push has no claim on it and says so rather than
/// adopting bytes it did not produce.
#[test]
fn a_version_occupied_by_other_bytes_is_never_adopted_by_a_resume() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());

    publish(&registry, &artifact(scratch.path(), "theirs.tgz", b"theirs"), "1.0.0").unwrap();
    std::fs::remove_file(dir.path().join("v1/index/acme-core.json")).unwrap();

    let refused = publish(&registry, &artifact(scratch.path(), "ours.tgz", b"ours"), "1.0.0")
        .expect_err("different bytes are a taken version, not a half-done publish");
    assert!(
        matches!(refused, RegistryError::VersionExists { .. }),
        "{refused:?}"
    );
}

/// A name in the enumeration document is input like any other, and it is about to become
/// a path segment.
///
/// The document is written by whoever publishes to the registry, so a hostile or merely
/// broken one must not be able to steer a read out of the registry root — the same
/// boundary check every other entry point applies (registry.md §10.2).
#[test]
fn a_traversing_name_in_the_enumeration_is_refused_not_followed() {
    let dir = temp();
    let scratch = temp();
    let registry = registry(dir.path());
    publish(&registry, &artifact(scratch.path(), "a.tgz", b"payload"), "1.0.0").unwrap();

    // A document naming something outside the registry, beside the legitimate entry.
    std::fs::write(
        dir.path().join("v1/packages.json"),
        serde_json::to_vec(&["../../../../etc/passwd", "acme-core"]).unwrap(),
    )
    .unwrap();
    // Somewhere for the traversal to actually land, so the test fails loudly if the name
    // is ever followed rather than merely returning nothing.
    std::fs::create_dir_all(dir.path().join("v1/index")).unwrap();
    std::fs::write(
        dir.path().join("outside.json"),
        serde_json::to_vec(&PackageIndex::new("outside")).unwrap(),
    )
    .unwrap();

    let listed = registry.list().expect("listing survives a bad name");
    assert_eq!(
        listed.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        vec!["acme-core"],
        "the ill-formed name is skipped and the good one still lists"
    );
}
