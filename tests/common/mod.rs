//! Shared test harness: build a throwaway Git-repo corpus, index it, query it.
//!
//! Indexing is bound to commit (commit-as-publish), so the harness writes files,
//! commits them, then builds — exactly the lifecycle the spec describes.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use vaire::commands::Ctx;
use vaire::config::Config;
use vaire::corpus::Repo;
use vaire::embed::Embedder;
use vaire::error::Result as VResult;
use vaire::index::build::{self, Mode};

/// A deterministic, network-free embedder for tests: every text maps to a fixed-width
/// zero vector. Graph queries never touch vectors, so this keeps `build` happy without
/// pulling in a real model.
pub struct DummyEmbedder {
    pub dims: usize,
}

impl Embedder for DummyEmbedder {
    fn embed(&self, texts: &[String]) -> VResult<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.0; self.dims]).collect())
    }
    fn dimensions(&self) -> usize {
        self.dims
    }
}

/// Like `DummyEmbedder` but counts how many texts it embeds — to prove the content-hash
/// cache skips unchanged sections on reindex.
pub struct CountingEmbedder {
    pub dims: usize,
    pub embedded: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Embedder for CountingEmbedder {
    fn embed(&self, texts: &[String]) -> VResult<Vec<Vec<f32>>> {
        self.embedded
            .fetch_add(texts.len(), std::sync::atomic::Ordering::Relaxed);
        // Vary the vector by text so different sections hash to different cache entries
        // is irrelevant here; the cache keys on text hash, not the vector.
        Ok(texts.iter().map(|_| vec![0.0; self.dims]).collect())
    }
    fn dimensions(&self) -> usize {
        self.dims
    }
    /// Distinct from `DummyEmbedder`'s identity, so tests can model a provider switch at
    /// identical dimensions — the case the `length(vector)` filter in search cannot catch.
    fn identity(&self) -> String {
        format!("counting:{}", self.dims)
    }
}

/// Point `VAIRE_CONFIG_HOME` at a per-process temp dir, once, before any fixture exists.
///
/// Commands that embed (`vaire index`'s ensure pass, `check --working-tree`) build their
/// embedder from the GLOBAL user config — without this, tests on a machine whose real
/// config says `provider = "openai"` would silently hit the network (and spend money)
/// from the test suite. An empty hermetic home means the default local provider, always.
fn hermetic_config_home() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("vaire-test-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("hermetic config home");
        // Safe in practice: every fixture constructor funnels through this Once before
        // any embedder is built, and the value is identical process-wide.
        unsafe { std::env::set_var("VAIRE_CONFIG_HOME", &dir) };
    });
}

pub struct Corpus {
    pub dir: tempfile::TempDir,
}

impl Corpus {
    /// A fresh, empty Git repo with a `knowledge.toml` marker (so discovery finds it). The
    /// declared `types` mirror the old default vocabulary so `check` behaves as before.
    pub fn empty() -> Self {
        hermetic_config_home();
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "test@vaire.test"]);
        git(dir.path(), &["config", "user.name", "Vaire Test"]);
        // Hermetic: never sign test commits, regardless of the developer's global git config.
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        std::fs::write(
            dir.path().join("knowledge.toml"),
            "name = \"test-corpus\"\nversion = \"0.1.0\"\n\
             types = [\"person\", \"department\", \"method\", \"system\", \"event\", \"record\", \"project\"]\n",
        )
        .unwrap();
        Corpus { dir }
    }

    /// Overwrite `knowledge.toml` with an explicit `[dependencies]` block appended to the
    /// default marker (for cross-package / `vaire add` tests). Chainable.
    pub fn with_dependencies(&self, deps: &[(&str, &str)]) -> &Self {
        let mut toml = String::from(
            "name = \"test-corpus\"\nversion = \"0.1.0\"\n\
             types = [\"person\", \"department\", \"method\", \"system\", \"event\", \"record\", \"project\"]\n",
        );
        if !deps.is_empty() {
            toml.push_str("\n[dependencies]\n");
            for (name, constraint) in deps {
                toml.push_str(&format!("{name} = \"{constraint}\"\n"));
            }
        }
        std::fs::write(self.dir.path().join("knowledge.toml"), toml).unwrap();
        self
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Write a file (creating parent dirs). Chainable.
    pub fn add(&self, rel: &str, contents: &str) -> &Self {
        let p = self.dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
        self
    }

    /// Stage and commit everything written so far. Chainable.
    pub fn commit(&self) -> &Self {
        git(self.dir.path(), &["add", "-A"]);
        git(self.dir.path(), &["commit", "-q", "-m", "snapshot"]);
        self
    }

    /// Run a full index build over the committed tree.
    pub fn build(&self) -> &Self {
        self.build_with(&DummyEmbedder { dims: 8 }, Mode::Full)
    }

    /// Build/reindex with a specific embedder and mode (for cache tests). Uses the
    /// corpus's actual manifest (as commands do), so `nodes.package`/`package_name`
    /// reflect the declared name.
    pub fn build_with(&self, embedder: &dyn Embedder, mode: Mode) -> &Self {
        let config = Config::load(&self.dir.path().join("knowledge.toml")).expect("manifest");
        self.build_cfg(&config, embedder, mode)
    }

    /// Build/reindex with an explicit config (for scoped-ID tests).
    pub fn build_cfg(&self, config: &Config, embedder: &dyn Embedder, mode: Mode) -> &Self {
        build::run(&self.repo(), config, embedder, mode).expect("index build");
        self
    }

    /// Re-embed the existing index with a specific embedder (for `--re-embed` tests).
    pub fn reembed_with(&self, embedder: &dyn Embedder) -> &Self {
        build::reembed(&self.repo(), embedder).expect("reembed");
        self
    }

    pub fn repo(&self) -> Repo {
        Repo::discover(Some(self.dir.path()), self.dir.path()).unwrap()
    }

    /// This corpus's `.vaire/packages` dir (for asserting link state).
    pub fn packages_dir(&self) -> std::path::PathBuf {
        self.dir.path().join(".vaire").join("packages")
    }

    /// A command context pointed at this corpus.
    pub fn ctx(&self) -> Ctx {
        Ctx::new(Some(self.dir.path().to_path_buf()), None).unwrap()
    }

    /// Build the standard spec fixture (the design.md/cli.md examples), commit, index.
    pub fn fixture() -> Self {
        let c = Corpus::empty();
        c.add(
            "knowledge/entities/people/jane-doe.md",
            r#"---
id: jane-doe
type: person
name: Jane Doe
aliases: [Jane, J. Doe]
org: department:platform
status: active
updated: 2026-06-15
---
# Jane Doe

Role at [[department:platform]]. Works on [[method:event-sourcing]] architecture.
"#,
        );
        // A superseded duplicate that redirects to jane-doe (design.md §8).
        c.add(
            "knowledge/entities/people/j-doe-dup.md",
            r#"---
id: j-doe-dup
type: person
name: J. Doe (dup)
status: superseded
superseded_by: person:jane-doe
---
# J. Doe (dup)
"#,
        );
        c.add(
            "knowledge/entities/departments/platform.md",
            "---\nid: platform\ntype: department\nname: Platform\n---\n# Platform\n",
        );
        c.add(
            "knowledge/entities/departments/logistics.md",
            "---\nid: logistics\ntype: department\nname: Logistics\naliases: [logistics contact]\n---\n# Logistics\n",
        );
        c.add(
            "knowledge/entities/methods/event-sourcing.md",
            "---\nid: event-sourcing\ntype: method\nname: Event Sourcing\n---\n# Event Sourcing\n",
        );
        c.add(
            "knowledge/entities/systems/ingest-api.md",
            "---\nid: ingest-api\ntype: system\nname: Ingest API\n---\n# Ingest API\n",
        );
        c.add(
            "knowledge/entities/projects/atlas-2026-q2.md",
            "---\nid: atlas-2026-q2\ntype: project\nname: Atlas Q2\n---\n# Atlas Q2\n",
        );
        c.add(
            "projects/atlas/2026_q2/decisions/2026-06-08-ingest-decision.md",
            r#"---
id: 2026-06-08-ingest-decision
type: record
scope: project:atlas-2026-q2
references: [system:ingest-api]
---
# Ingest decision

Scope [[system:ingest-api]] first.
"#,
        );
        c.add(
            "projects/atlas/2026_q2/meeting-notes/2026-06-10-broker-sync.md",
            r#"---
id: 2026-06-10-broker-sync
type: record
scope: project:atlas-2026-q2
date: 2026-06-10
participants: [person:jane-doe, department:logistics]
references: [method:event-sourcing, system:ingest-api]
---
# Broker sync, 2026-06-10

[[person:jane-doe]] walked [[department:logistics]] through partition segmentation.
Decision to scope [[system:ingest-api]] first — see [[record:2026-06-08-ingest-decision]].
[[?person: someone from logistics]] raised throughput concerns about the [[?: the broker thing]].
"#,
        );
        c.commit().build();
        c
    }
}

/// A multi-package workspace: sibling package dirs under one tempdir, linked via
/// `.vaire/packages` (cli.md §6.5). Each member is its own hermetic git repo built with
/// the `DummyEmbedder`, exactly like [`Corpus`] but plural.
pub struct Ws {
    pub dir: tempfile::TempDir,
}

#[allow(unused)]
impl Ws {
    pub fn new() -> Self {
        hermetic_config_home();
        Ws {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    pub fn root(&self, pkg: &str) -> PathBuf {
        self.dir.path().join(pkg)
    }

    /// Create a member package: its own git repo + a manifest declaring `name`, the given
    /// `types`, and `[dependencies]`.
    pub fn add_package(&self, name: &str, types: &[&str], deps: &[(&str, &str)]) -> &Self {
        self.add_package_named(name, name, types, deps)
    }

    /// Like [`Ws::add_package`] but the directory name and the **declared** name differ —
    /// identity is declared, never path-derived, so tests can prove matching goes by the
    /// manifest.
    pub fn add_package_named(
        &self,
        dir: &str,
        name: &str,
        types: &[&str],
        deps: &[(&str, &str)],
    ) -> &Self {
        let root = self.root(dir);
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "test@vaire.test"]);
        git(&root, &["config", "user.name", "Vaire Test"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        let types_toml = types
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let mut manifest =
            format!("name = \"{name}\"\nversion = \"1.0.0\"\ntypes = [{types_toml}]\n");
        if !deps.is_empty() {
            manifest.push_str("\n[dependencies]\n");
            for (dep, constraint) in deps {
                manifest.push_str(&format!("{dep} = \"{constraint}\"\n"));
            }
        }
        std::fs::write(root.join("knowledge.toml"), manifest).unwrap();
        self
    }

    /// Write a file into a member (creating parent dirs). Chainable.
    pub fn add_file(&self, pkg: &str, rel: &str, contents: &str) -> &Self {
        let p = self.root(pkg).join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
        self
    }

    /// Stage and commit everything in a member. Chainable.
    pub fn commit(&self, pkg: &str) -> &Self {
        git(&self.root(pkg), &["add", "-A"]);
        git(&self.root(pkg), &["commit", "-q", "-m", "snapshot"]);
        self
    }

    /// Link `dep` into `pkg` via the real `vaire add --link` (also normalizes the
    /// manifest constraint to `^1` if absent — idempotent).
    pub fn link(&self, pkg: &str, dep: &str) -> &Self {
        self.link_to(pkg, dep, dep)
    }

    /// Link dependency `dep_name` of `pkg` to a specific member directory (which must
    /// declare `dep_name` — the real `vaire add --link` validates that).
    pub fn link_to(&self, pkg: &str, dep_name: &str, target_dir: &str) -> &Self {
        vaire::commands::add::run(
            Some(&self.root(pkg)),
            None,
            dep_name,
            Some(&self.root(target_dir)),
        )
        .expect("add --link");
        self
    }

    /// Full index build of one member with the dummy embedder (like `Corpus::build`).
    pub fn build(&self, pkg: &str) -> &Self {
        let root = self.root(pkg);
        let repo = Repo::discover(Some(&root), &root).unwrap();
        let config = Config::load(&root.join("knowledge.toml")).unwrap();
        build::run(&repo, &config, &DummyEmbedder { dims: 8 }, Mode::Full).expect("index build");
        self
    }

    /// A command context rooted at one member.
    pub fn ctx(&self, pkg: &str) -> Ctx {
        Ctx::new(Some(self.root(pkg)), None).unwrap()
    }

    /// The acceptance-criteria workspace (issue #2): acme-core [team, person] and
    /// acme-web [service] in a dependency cycle, acme-shared [site] a leaf. acme-web
    /// links both deps; acme-core deliberately links nothing (exercising the run-root
    /// fallback and the run-root-itself case). Includes a cross-package tombstone
    /// (`service:legacy` → `@acme-core/team:platform`) and a deliberately dangling
    /// `@acme-shared/wiki:home` reference. All members committed and indexed.
    pub fn acceptance() -> Self {
        let ws = Ws::new();
        ws.add_package("acme-core", &["team", "person"], &[("acme-web", "^1")])
            .add_file(
                "acme-core",
                "knowledge/platform.md",
                "---\nid: platform\ntype: team\nname: Platform Team\nflagship: \"@acme-web/service:checkout\"\n---\n# Platform Team\n\nLed by [[person:jane-doe]]. Ships [[@acme-web/service:checkout]].\n\nContact [[?person: the incident manager]].\n",
            )
            .add_file(
                "acme-core",
                "knowledge/jane.md",
                "---\nid: jane-doe\ntype: person\nname: Jane Doe\n---\n# Jane Doe\n",
            )
            .commit("acme-core");

        ws.add_package("acme-shared", &["site", "person"], &[])
            .add_file(
                "acme-shared",
                "knowledge/hq.md",
                "---\nid: hq\ntype: site\nname: Headquarters\n---\n# Headquarters\n",
            )
            .commit("acme-shared");

        ws.add_package(
            "acme-web",
            &["service"],
            &[("acme-core", "^1"), ("acme-shared", "^1")],
        )
        .add_file(
            "acme-web",
            "knowledge/checkout.md",
            "---\nid: checkout\ntype: service\nname: Checkout\nowner: \"@acme-core/team:platform\"\nsite: \"@acme-shared/site:hq\"\nwiki: \"@acme-shared/wiki:home\"\n---\n# Checkout\n\nOwned by [[@acme-core/team:platform]] at [[@acme-shared/site:hq|HQ]].\n\nEscalation: [[?person: the on-call lead]].\n",
        )
        .add_file(
            "acme-web",
            "knowledge/legacy.md",
            "---\nid: legacy\ntype: service\nname: Legacy\nsuperseded_by: \"@acme-core/team:platform\"\n---\n# Legacy\n",
        )
        .commit("acme-web");

        // acme-web links both dependencies; acme-core stays link-less on purpose.
        ws.link("acme-web", "acme-core")
            .link("acme-web", "acme-shared");
        // Re-commit acme-web: `vaire add --link` normalized its manifest.
        ws.commit("acme-web");

        ws.build("acme-core").build("acme-shared").build("acme-web");
        ws
    }
}

/// HEAD commit SHA of `root`, for assertions about commit-bound indexing.
pub fn head(root: &Path) -> String {
    let out = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

pub fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

/// Force `tests/common` to be a module even when a test file uses only part of it.
#[allow(unused)]
pub fn _used(_: PathBuf) {}
