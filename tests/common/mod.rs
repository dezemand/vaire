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
}

pub struct Corpus {
    pub dir: tempfile::TempDir,
}

impl Corpus {
    /// A fresh, empty Git repo with a `.vaire/` dir (so discovery finds it).
    pub fn empty() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "test@vaire.test"]);
        git(dir.path(), &["config", "user.name", "Vaire Test"]);
        std::fs::create_dir_all(dir.path().join(".vaire")).unwrap();
        Corpus { dir }
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

    /// Build/reindex with a specific embedder and mode (for cache tests).
    pub fn build_with(&self, embedder: &dyn Embedder, mode: Mode) -> &Self {
        self.build_cfg(&Config::default(), embedder, mode)
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

fn git(root: &Path, args: &[&str]) {
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
