//! `pin` / `unpin`, `clean`, and the adopted-changes digest (registry.md §5).
//!
//! The store's default behavior is to move: retention replaces a version within its major
//! line, resolution takes the highest satisfying one, and a sweep removes what nothing
//! needs. These are the three commands that let somebody say *not this*, and the digest
//! that explains what moving cost.
//!
//! The fixtures are the store suite's, because none of these questions mean anything
//! without a real publish → pull loop behind them.

mod common;

use std::path::Path;

use common::Corpus;
use vaire::lockfile::Lockfile;
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

    /// Edit an existing entity and commit it, so the next release records a `changed` edge.
    fn revise(&self, rel: &str, contents: &str) -> &Publisher {
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

/// A consumer package that depends on the glossary, with its own hermetic vaire home.
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

    fn root(&self) -> &Path {
        self.corpus.root()
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

    fn index(&self) -> vaire::output::IndexRunOutput {
        vaire::commands::index::run(&self.ctx(), false, false, false, false).expect("index")
    }

    fn pin(&self, spec: &str) -> vaire::Result<vaire::output::PinOutput> {
        vaire::commands::pin::pin(&self.ctx(), spec)
    }

    fn unpin(&self, name: &str) -> vaire::Result<vaire::output::PinOutput> {
        vaire::commands::pin::unpin(&self.ctx(), name)
    }

    fn clean(&self, package: Option<&str>, dry_run: bool) -> vaire::output::CleanOutput {
        vaire::commands::clean::run(
            self.home.path(),
            vaire::commands::clean::Options { package, dry_run },
        )
        .expect("clean runs")
    }

    fn lockfile(&self) -> Option<Lockfile> {
        Lockfile::load(self.corpus.root()).expect("lockfile reads")
    }

    /// Record this package in the catalog, which is what the binary's ambient registration
    /// does around every maintain command (`main.rs`) and what `clean` needs in order to
    /// find this lockfile at all.
    fn register(&self) -> &Consumer {
        vaire::commands::catalog::add(self.home.path(), Some(self.corpus.root()))
            .expect("catalog add");
        self
    }

    /// Catalog the publisher's working copy on this machine — the two-worlds situation.
    fn also_knows_the_working_copy(&self, publisher: &Publisher) -> &Consumer {
        vaire::commands::catalog::add(self.home.path(), Some(publisher.root()))
            .expect("catalog add");
        self
    }
}

/// Sealed entries refuse deletion, so the temp home has to be unsealed before it can go.
impl Drop for Consumer {
    fn drop(&mut self) {
        let _ = vaire::store::unseal(self.home.path());
    }
}

/// The same guarantee for a bare vaire home with no `Consumer` around it. A panicking
/// assertion would otherwise leave the store sealed and defeat the `TempDir` cleanup, which
/// is the leak `Consumer`'s own `Drop` exists to prevent.
struct Home(tempfile::TempDir);

impl Home {
    fn new() -> Home {
        Home(temp())
    }

    fn path(&self) -> &Path {
        self.0.path()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = vaire::store::unseal(self.0.path());
    }
}

/// A publisher that has cut 1.0.0 and 1.1.0, and a consumer whose store holds **only
/// 1.0.0** — 1.1.0 is published and not yet pulled, which is what lets a test ask what
/// happens when somebody names a version this machine has never seen.
fn two_versions(registry: &Path) -> (Publisher, Consumer) {
    let publisher = Publisher::new(registry);
    publisher.publish();

    let consumer = Consumer::new(registry);
    consumer.pull(None);
    assert!(consumer.store().has("acme-glossary", Version::new(1, 0, 0)));

    publisher
        .grow(
            "knowledge/slip.md",
            "---\nid: slip-angle\ntype: term\nname: Slip angle\n---\n\
             # Slip angle\n\nThe angle between heading and travel.\n",
        )
        .publish();
    (publisher, consumer)
}

// ---- what a pin holds --------------------------------------------------------------------

#[test]
fn a_pin_survives_the_retention_that_would_otherwise_replace_it() {
    let registry = temp();
    let (_publisher, consumer) = two_versions(registry.path());

    consumer.pin("acme-glossary@1.0.0").expect("pin");
    // Retention is one slot per major line, so without the pin this pull removes 1.0.0.
    consumer.pull(Some("acme-glossary"));

    assert!(consumer.store().has("acme-glossary", Version::new(1, 1, 0)));
    assert!(
        consumer.store().has("acme-glossary", Version::new(1, 0, 0)),
        "a pin that retention could clear would not be a pin"
    );
}

#[test]
fn resolution_takes_the_pinned_version_over_anything_newer() {
    let registry = temp();
    let (_publisher, consumer) = two_versions(registry.path());
    // Pinned first, so retention keeps 1.0.0 when 1.1.0 arrives — a pin can only hold what
    // is still there.
    consumer.pin("acme-glossary@1.0.0").expect("pin");
    consumer.pull(Some("acme-glossary"));

    // Links are only re-selected when the entry is not already resolvable, so the stale one
    // has to go first — which is exactly what happens on a fresh clone.
    let _ = std::fs::remove_file(consumer.root().join(".vaire/packages/acme-glossary"));
    let indexed = consumer.index();

    let dep = indexed
        .dependencies
        .iter()
        .find(|dep| dep.name == "acme-glossary")
        .expect("in the closure");
    let linked = dep.linked.as_deref().expect("the pass linked it afresh");
    assert!(
        linked.contains("1.0.0"),
        "the pin selects within the store: {dep:?}"
    );
}

#[test]
fn a_pin_is_recorded_where_a_colleague_would_find_it() {
    let registry = temp();
    let (_publisher, consumer) = two_versions(registry.path());
    consumer.pin("acme-glossary@1.0.0").expect("pin");

    let locked = consumer.lockfile().expect("written");
    let entry = locked.get("acme-glossary").expect("recorded");
    assert!(entry.pinned);
    assert_eq!(entry.version, Version::new(1, 0, 0));
    // The digest comes from the artifact the entry was materialized from, never from the
    // version somebody typed — so the pin is reproducible like any other registry entry.
    assert!(entry.reproducible(), "{entry:?}");
}

#[test]
fn a_pin_does_not_displace_a_working_copy_but_says_so() {
    let registry = temp();
    let (publisher, consumer) = two_versions(registry.path());
    consumer.also_knows_the_working_copy(&publisher);

    let out = consumer.pin("acme-glossary@1.0.0").expect("pin");
    // Resolution order is untouched by a pin (§6): a hold on a version is not a statement
    // about which *world* answers, and displacing a checkout would mean cloning a package
    // with a pinned lockfile silently stopped using your own copy of its dependency.
    assert!(
        out.warnings.iter().any(|w| w.contains("working copy")),
        "an inert pin is worth one line now rather than a puzzle later: {out:?}"
    );
    // Still recorded, because on a machine without that checkout it is what resolves.
    assert!(
        consumer
            .lockfile()
            .unwrap()
            .get("acme-glossary")
            .unwrap()
            .pinned
    );
}

#[test]
fn unpinning_lets_retention_move_again() {
    let registry = temp();
    let (_publisher, consumer) = two_versions(registry.path());
    consumer.pin("acme-glossary@1.0.0").expect("pin");
    consumer.unpin("acme-glossary").expect("unpin");

    consumer.pull(Some("acme-glossary"));
    assert!(!consumer.store().has("acme-glossary", Version::new(1, 0, 0)));
    // The entry itself survives the unpin — what it records is still what resolved, and
    // dropping it would erase a reproducible answer in order to release a hold on it.
    assert!(consumer.lockfile().unwrap().get("acme-glossary").is_some());
}

// ---- what a pin refuses ------------------------------------------------------------------

#[test]
fn you_can_only_pin_what_the_store_actually_holds() {
    let registry = temp();
    let (_publisher, consumer) = two_versions(registry.path());

    // A lockfile entry is a claim about bytes, carrying their digest. Writing one for a
    // release this machine has never seen would put a reproducibility claim in a committed
    // file on the strength of a version number.
    let e = consumer.pin("acme-glossary@1.1.0").unwrap_err().to_string();
    assert!(e.contains("vaire pull acme-glossary@1.1.0"), "{e}");
    assert!(
        !consumer
            .lockfile()
            .unwrap()
            .get("acme-glossary")
            .unwrap()
            .pinned
    );
}

#[test]
fn a_pin_outside_the_declared_major_line_is_refused_at_the_door() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();
    let consumer = Consumer::new(registry.path());
    consumer.pull(None);

    // The useful moment to report a contradiction is now, not at the next resolution where
    // the pin would simply appear not to work.
    let e = consumer.pin("acme-glossary@2.0.0").unwrap_err().to_string();
    assert!(e.contains("^1"), "{e}");
    assert!(e.contains("major line"), "{e}");
}

#[test]
fn pinning_something_undeclared_is_refused() {
    let registry = temp();
    let (_publisher, consumer) = two_versions(registry.path());

    let e = consumer.pin("acme-other@1.0.0").unwrap_err().to_string();
    assert!(e.contains("not a dependency"), "{e}");
}

#[test]
fn unpinning_what_is_not_pinned_says_so() {
    let registry = temp();
    let (_publisher, consumer) = two_versions(registry.path());
    let e = consumer.unpin("acme-glossary").unwrap_err().to_string();
    assert!(e.contains("not pinned"), "{e}");
}

// ---- clean -------------------------------------------------------------------------------

#[test]
fn clean_keeps_what_a_lockfile_names_and_takes_the_leftover() {
    let registry = temp();
    let (_publisher, consumer) = two_versions(registry.path());
    one_rooted_one_leftover(&consumer);

    let out = consumer.clean(None, false);
    assert_eq!(out.removed.len(), 1, "{out:?}");
    assert_eq!(out.removed[0].version, "1.1.0", "{out:?}");
    assert!(
        consumer.store().has("acme-glossary", Version::new(1, 0, 0)),
        "the lockfile names it, and that record is what somebody reproduces from"
    );
}

/// Leave the consumer holding two versions, one of them rooted by the lockfile and one of
/// them a leftover — the state a sweep is for. Pinning is what stops retention from tidying
/// the older one away before `clean` can be asked about it.
fn one_rooted_one_leftover(consumer: &Consumer) {
    consumer.register();
    consumer.index(); // the lockfile now names 1.0.0
    consumer.pin("acme-glossary@1.0.0").expect("pin");
    consumer.pull(Some("acme-glossary")); // 1.1.0 arrives; the pin keeps 1.0.0
    consumer.unpin("acme-glossary").expect("unpin");
    assert_eq!(consumer.store().entries().len(), 2);
}

#[test]
fn a_dry_run_reports_the_same_sweep_and_deletes_nothing() {
    let registry = temp();
    let (_publisher, consumer) = two_versions(registry.path());
    one_rooted_one_leftover(&consumer);

    let dry = consumer.clean(None, true);
    assert!(dry.dry_run);
    assert_eq!(dry.removed.len(), 1, "{dry:?}");
    assert!(consumer.store().has("acme-glossary", Version::new(1, 1, 0)));

    let wet = consumer.clean(None, false);
    assert_eq!(
        wet.removed.len(),
        dry.removed.len(),
        "a dry run that described a different sweep would be worthless"
    );
    assert!(!consumer.store().has("acme-glossary", Version::new(1, 1, 0)));
}

#[test]
fn a_package_pulled_by_name_survives_a_sweep_until_it_is_named_again() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    // A rootless reader: pulled by name from outside any package, so no lockfile records
    // it anywhere. Rooting only what a lockfile names would delete a reader's whole corpus.
    let home = Home::new();
    let ctx = vaire::commands::Ctx::rootless(home.path().to_path_buf()).expect("rootless");
    vaire::commands::registry::add(
        home.path(),
        "lab",
        &registry.path().display().to_string(),
        0,
        true,
    )
    .expect("registry add");
    vaire::commands::pull::run(
        &ctx,
        vaire::commands::pull::Options {
            spec: Some("acme-glossary"),
            registry: None,
            locked: false,
            dry_run: false,
        },
    )
    .expect("pull");
    let store = Store::at(home.path());
    assert!(store.has("acme-glossary", Version::new(1, 0, 0)));

    let kept = vaire::commands::clean::run(
        home.path(),
        vaire::commands::clean::Options {
            package: None,
            dry_run: false,
        },
    )
    .expect("clean");
    assert!(kept.removed.is_empty(), "{kept:?}");
    assert!(store.has("acme-glossary", Version::new(1, 0, 0)));

    // Naming it is how the request is withdrawn — the one way to say "I am done with this".
    let swept = vaire::commands::clean::run(
        home.path(),
        vaire::commands::clean::Options {
            package: Some("acme-glossary"),
            dry_run: false,
        },
    )
    .expect("clean");
    assert_eq!(swept.removed.len(), 1, "{swept:?}");
    assert!(!store.has("acme-glossary", Version::new(1, 0, 0)));
}

#[test]
fn clean_refuses_to_sweep_while_a_lockfile_cannot_be_read() {
    let registry = temp();
    let (_publisher, consumer) = two_versions(registry.path());
    consumer.register();
    consumer.index();
    // Pinned, so both versions stay in the store and there is something for the refused
    // sweep to have protected.
    consumer.pin("acme-glossary@1.0.0").expect("pin");
    consumer.pull(Some("acme-glossary"));

    // Refusing to read a lockfile and then treating it as holding nothing would delete
    // exactly the entries it was protecting.
    std::fs::write(
        consumer.root().join("knowledge.lock"),
        "lockfile_version = 99\n",
    )
    .unwrap();
    let e = vaire::commands::clean::run(
        consumer.home.path(),
        vaire::commands::clean::Options {
            package: None,
            dry_run: false,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("could not be read"), "{e}");
    assert_eq!(consumer.store().entries().len(), 2, "nothing was swept");
}

// ---- the adopted-changes digest ----------------------------------------------------------

#[test]
fn advancing_a_dependency_reports_what_changed_that_this_package_cites() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    consumer.pull(None);
    consumer.index();

    // Two changes in the next release: one the consumer references, one it has never heard
    // of. The digest is the intersection, which is the whole point — a changelog would
    // report both.
    publisher
        .revise(
            "knowledge/torque.md",
            "---\nid: torque-vectoring\ntype: term\nname: Torque vectoring\n---\n\
             # Torque vectoring\n\nDistributing drive torque between wheels, per axle.\n\
             See also the yaw response.\n",
        )
        .grow(
            "knowledge/slip.md",
            "---\nid: slip-angle\ntype: term\nname: Slip angle\n---\n\
             # Slip angle\n\nThe angle between heading and travel.\n",
        )
        .publish();

    let out = consumer.pull(Some("acme-glossary"));
    let release = out
        .pulled
        .iter()
        .find(|release| release.package == "acme-glossary")
        .expect("pulled");
    let adopted = release
        .adopted
        .as_ref()
        .expect("a version that replaced one has a range to report over");

    assert_eq!(adopted.from, "1.0.0");
    assert!(adopted.touched >= 2, "{adopted:?}");
    let cited: Vec<&str> = adopted
        .cited
        .iter()
        .map(|change| change.id.as_str())
        .collect();
    assert_eq!(
        cited,
        ["term:torque-vectoring"],
        "only what this package actually references: {adopted:?}"
    );
}

#[test]
fn a_first_pull_adopts_nothing_because_there_is_no_range() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let consumer = Consumer::new(registry.path());
    let out = consumer.pull(None);
    assert!(
        out.pulled[0].adopted.is_none(),
        "arriving somewhere for the first time adopts nothing"
    );
}

#[test]
fn re_pinning_lets_retention_take_the_version_that_was_held_before() {
    let registry = temp();
    let (publisher, consumer) = two_versions(registry.path());
    consumer.pin("acme-glossary@1.0.0").expect("pin");
    consumer.pull(Some("acme-glossary")); // 1.1.0 arrives; the pin keeps 1.0.0
    consumer.pin("acme-glossary@1.1.0").expect("re-pin");

    publisher
        .grow(
            "knowledge/yaw.md",
            "---\nid: yaw-rate\ntype: term\nname: Yaw rate\n---\n\
             # Yaw rate\n\nRotation about the vertical axis.\n",
        )
        .publish();
    consumer.pull(Some("acme-glossary"));

    // The hold moved, so what it used to hold is ordinary again. Leaving the old version
    // flagged would have retention keep it for good, on the strength of a pin no lockfile
    // still records.
    assert!(
        !consumer.store().has("acme-glossary", Version::new(1, 0, 0)),
        "the pin that used to hold 1.0.0 moved off it"
    );
    assert!(
        consumer.store().has("acme-glossary", Version::new(1, 1, 0)),
        "the pin that now holds 1.1.0 keeps it"
    );
}

#[test]
fn a_refused_sweep_does_not_withdraw_the_request_it_refused_to_act_on() {
    let registry = temp();
    let publisher = Publisher::new(registry.path());
    publisher.publish();

    let home = Home::new();
    vaire::commands::registry::add(
        home.path(),
        "lab",
        &registry.path().display().to_string(),
        0,
        true,
    )
    .expect("registry add");
    let ctx = vaire::commands::Ctx::rootless(home.path().to_path_buf()).expect("rootless");
    vaire::commands::pull::run(
        &ctx,
        vaire::commands::pull::Options {
            spec: Some("acme-glossary"),
            registry: None,
            locked: false,
            dry_run: false,
        },
    )
    .expect("pull");

    // A registered package on the same machine whose lockfile this vaire will not read.
    let other = temp();
    std::fs::write(
        other.path().join("knowledge.toml"),
        "name = \"acme-other\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        other.path().join("knowledge.lock"),
        "lockfile_version = 99\n",
    )
    .unwrap();
    vaire::commands::catalog::add(home.path(), Some(other.path())).expect("catalog add");

    let clean = |package: Option<&str>| {
        vaire::commands::clean::run(
            home.path(),
            vaire::commands::clean::Options {
                package,
                dry_run: false,
            },
        )
    };
    assert!(
        clean(Some("acme-glossary")).is_err(),
        "the sweep is refused"
    );

    // The refusal said the sweep did not happen. If the request had been withdrawn anyway,
    // the next successful sweep would take a package the user never named a second time.
    std::fs::remove_file(other.path().join("knowledge.lock")).unwrap();
    let out = clean(None).expect("clean");
    assert!(out.removed.is_empty(), "{out:?}");
    assert!(Store::at(home.path()).has("acme-glossary", Version::new(1, 0, 0)));
}
