//! Spec tests for `vaire init` — scaffold or migrate a package so it becomes discoverable.

mod common;

use common::DummyEmbedder;
use vaire::commands;
use vaire::config::Config;
use vaire::corpus::Repo;
use vaire::index::build::{self, Mode};

#[test]
fn init_writes_manifest_and_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    let out = commands::init::run(Some(dir.path())).unwrap();
    assert!(!out.migrated);

    // The v0.2 marker is `knowledge.toml` at the root; `.vaire/` is derived-only.
    assert!(dir.path().join("knowledge.toml").exists());
    assert!(dir.path().join(".vaire/.gitignore").exists());

    // The written manifest is valid and loads (name + version present).
    Config::load(&dir.path().join("knowledge.toml")).unwrap();
    // The directory is now discoverable as a package root.
    assert!(Repo::discover(Some(dir.path()), dir.path()).is_ok());
}

#[test]
fn init_refuses_to_clobber_existing_package() {
    let dir = tempfile::tempdir().unwrap();
    commands::init::run(Some(dir.path())).unwrap();
    // A second init must not overwrite an existing knowledge.toml.
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

#[test]
fn init_migrates_legacy_vaire_config() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".vaire")).unwrap();
    std::fs::write(
        dir.path().join(".vaire/config.toml"),
        "include = [\"knowledge/**/*.md\"]\n\
         id_prefixes = [\"person\", \"service\"]\n\
         scoped_types = [\"record\"]\n\
         vocabulary_strict = true\n\n\
         [embeddings]\nprovider = \"local\"\ndimensions = 384\n",
    )
    .unwrap();

    let out = commands::init::run(Some(dir.path())).unwrap();
    assert!(out.migrated);

    // New manifest written; the legacy file is set aside, not left in place.
    assert!(dir.path().join("knowledge.toml").exists());
    assert!(dir.path().join(".vaire/config.toml.migrated").exists());
    assert!(!dir.path().join(".vaire/config.toml").exists());

    // Transformed: id_prefixes → types, embeddings dropped, name/version injected, rest kept.
    let cfg = Config::load(&dir.path().join("knowledge.toml")).unwrap();
    assert_eq!(cfg.types, vec!["person", "service"]);
    assert!(cfg.vocabulary_strict);
    assert!(!cfg.name.is_empty());
    assert_eq!(cfg.version, "0.1.0");
    // Legacy scoped_types becomes the whitelist lint policy.
    assert_eq!(cfg.scoped_types_whitelist, vec!["record"]);

    let text = std::fs::read_to_string(dir.path().join("knowledge.toml")).unwrap();
    assert!(
        !text.contains("[embeddings]"),
        "embeddings should be dropped"
    );
    assert!(
        !text.contains("id_prefixes"),
        "id_prefixes should be renamed to types"
    );
}

#[test]
fn init_refuses_when_knowledge_toml_already_present() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("knowledge.toml"),
        "name = \"already\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    // Even with a legacy config alongside, an existing knowledge.toml is never overwritten.
    std::fs::create_dir_all(dir.path().join(".vaire")).unwrap();
    std::fs::write(
        dir.path().join(".vaire/config.toml"),
        "id_prefixes = [\"x\"]\n",
    )
    .unwrap();
    assert!(commands::init::run(Some(dir.path())).is_err());
}

#[test]
fn discover_on_a_legacy_corpus_points_to_init() {
    // A dir with the old .vaire/config.toml but no knowledge.toml: discovery should not
    // say "no corpus" — it should tell the user to run `vaire init` to migrate.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".vaire")).unwrap();
    std::fs::write(
        dir.path().join(".vaire/config.toml"),
        "id_prefixes = [\"x\"]\n",
    )
    .unwrap();

    let err = Repo::discover(Some(dir.path()), dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("migrate"),
        "legacy corpus error should point to `vaire init`, got: {err}"
    );
}

#[test]
fn discover_walkup_finds_legacy_config_and_points_to_init() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".vaire")).unwrap();
    std::fs::write(
        dir.path().join(".vaire/config.toml"),
        "id_prefixes = [\"x\"]\n",
    )
    .unwrap();
    let sub = dir.path().join("a/b");
    std::fs::create_dir_all(&sub).unwrap();

    let err = Repo::discover(None, &sub).unwrap_err();
    assert!(err.to_string().contains("migrate"), "got: {err}");
}
