//! Spec tests for `vaire add <pkg>[@^N]` — declare a dependency in knowledge.toml,
//! preserving the user's formatting and comments (manifest.md §5).

mod common;

use common::Corpus;
use vaire::commands;
use vaire::config::Config;

/// Read the corpus manifest back as raw text.
fn manifest(c: &Corpus) -> String {
    std::fs::read_to_string(c.root().join("knowledge.toml")).unwrap()
}

#[test]
fn add_writes_a_default_caret_one_dependency() {
    let c = Corpus::empty();
    let out = commands::add::run(Some(c.root()), None, "acme-core", None).unwrap();
    assert_eq!(out.name, "acme-core");
    assert_eq!(out.constraint, "^1");
    assert!(!out.updated);

    // It parses as a real dependency the manifest loader accepts.
    let cfg = Config::load(&c.root().join("knowledge.toml")).unwrap();
    assert_eq!(
        cfg.dependencies.get("acme-core").map(String::as_str),
        Some("^1")
    );
}

#[test]
fn add_honours_an_explicit_caret_major() {
    let c = Corpus::empty();
    let out = commands::add::run(Some(c.root()), None, "acme-web@^2", None).unwrap();
    assert_eq!(out.constraint, "^2");
    let cfg = Config::load(&c.root().join("knowledge.toml")).unwrap();
    assert_eq!(
        cfg.dependencies.get("acme-web").map(String::as_str),
        Some("^2")
    );
}

#[test]
fn add_is_idempotent_and_updates_in_place() {
    let c = Corpus::empty();
    commands::add::run(Some(c.root()), None, "acme-core@^1", None).unwrap();
    let out = commands::add::run(Some(c.root()), None, "acme-core@^2", None).unwrap();
    assert!(out.updated, "second add of same package updates in place");

    let cfg = Config::load(&c.root().join("knowledge.toml")).unwrap();
    assert_eq!(
        cfg.dependencies.get("acme-core").map(String::as_str),
        Some("^2")
    );
    // Exactly one entry — not duplicated.
    assert_eq!(manifest(&c).matches("acme-core").count(), 1);
}

#[test]
fn add_preserves_comments_and_formatting() {
    let c = Corpus::empty();
    // A user-authored manifest with a comment above the types line.
    c.add(
        "knowledge.toml",
        "name = \"test-corpus\"\nversion = \"0.1.0\"\n\n# our entity vocabulary\ntypes = [\"person\"]\n",
    );
    commands::add::run(Some(c.root()), None, "acme-core", None).unwrap();

    let text = manifest(&c);
    assert!(
        text.contains("# our entity vocabulary"),
        "comment preserved: {text}"
    );
    assert!(text.contains("[dependencies]"));
    assert!(text.contains("acme-core = \"^1\""));
}

#[test]
fn add_rejects_a_bad_name_or_constraint() {
    let c = Corpus::empty();
    // Uppercase / non-slug name.
    assert!(commands::add::run(Some(c.root()), None, "Acme_Core", None).is_err());
    // A tighter pin than ^MAJOR is not allowed.
    assert!(commands::add::run(Some(c.root()), None, "acme-core@1.2.3", None).is_err());
    assert!(commands::add::run(Some(c.root()), None, "acme-core@^", None).is_err());
}
