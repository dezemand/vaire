//! Spec tests for `vaire init` — scaffold a corpus so it becomes discoverable.

mod common;

use common::DummyEmbedder;
use vaire::commands;
use vaire::config::Config;
use vaire::corpus::Repo;
use vaire::index::build::{self, Mode};

#[test]
fn init_writes_marker_and_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    commands::init::run(Some(dir.path())).unwrap();

    assert!(dir.path().join(".vaire/config.toml").exists());
    assert!(dir.path().join(".vaire/.gitignore").exists());

    // The written config is valid and loads.
    Config::load(&dir.path().join(".vaire/config.toml")).unwrap();
    // The directory is now discoverable as a corpus root.
    assert!(Repo::discover(Some(dir.path()), dir.path()).is_ok());
}

#[test]
fn init_refuses_to_clobber_existing_corpus() {
    let dir = tempfile::tempdir().unwrap();
    commands::init::run(Some(dir.path())).unwrap();
    assert!(commands::init::run(Some(dir.path())).is_err());
}

#[test]
fn init_then_index_a_fresh_non_git_corpus() {
    // The full bootstrap path: init → write a node → index (working tree) → query.
    let dir = tempfile::tempdir().unwrap();
    commands::init::run(Some(dir.path())).unwrap();
    std::fs::create_dir_all(dir.path().join("knowledge")).unwrap();
    std::fs::write(
        dir.path().join("knowledge/x.md"),
        "---\nid: x\ntype: method\nname: Method X\n---\n# X\n",
    )
    .unwrap();

    let repo = Repo::discover(Some(dir.path()), dir.path()).unwrap();
    build::run(
        &repo,
        &Config::default(),
        &DummyEmbedder { dims: 8 },
        Mode::Full,
    )
    .unwrap();

    let ctx = commands::Ctx::new(Some(dir.path().to_path_buf()), None).unwrap();
    let out = commands::resolve::run(&ctx, "method:x").unwrap();
    assert_eq!(out.id, "method:x");
}
