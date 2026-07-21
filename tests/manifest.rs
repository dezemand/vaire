//! Spec tests for the `knowledge.toml` package manifest (v0.2.0 M1).
//!
//! The manifest adds package identity (`name`, `version`), `[dependencies]`, and renames
//! `id_prefixes` → `types`. Loading validates: `name` is a slug, `version` is semver, and
//! every dependency constraint is the `^MAJOR` form.

use vaire::config::Config;

/// Write `body` to a `knowledge.toml` in a fresh tempdir and load it.
fn load(body: &str) -> Result<Config, vaire::error::VaireError> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("knowledge.toml");
    std::fs::write(&path, body).unwrap();
    Config::load(&path)
}

#[test]
fn parses_a_full_manifest() {
    let cfg = load(
        r#"
name = "acme-web"
version = "1.4.2"
description = "The web application package"
types = ["service"]

[dependencies]
acme-core = "^1"
acme-ui = "^1"
"#,
    )
    .unwrap();

    assert_eq!(cfg.name, "acme-web");
    assert_eq!(cfg.version, "1.4.2");
    assert_eq!(
        cfg.description.as_deref(),
        Some("The web application package")
    );
    assert_eq!(cfg.types, vec!["service"]);
    assert_eq!(
        cfg.dependencies.get("acme-core").map(String::as_str),
        Some("^1")
    );
    assert_eq!(
        cfg.dependencies.get("acme-ui").map(String::as_str),
        Some("^1")
    );
}

#[test]
fn accepts_a_minimal_manifest() {
    let cfg = load("name = \"acme-core\"\nversion = \"1.0.0\"\n").unwrap();
    assert_eq!(cfg.name, "acme-core");
    assert_eq!(cfg.version, "1.0.0");
    assert!(cfg.types.is_empty());
    assert!(cfg.dependencies.is_empty());
    assert!(cfg.description.is_none());
}

#[test]
fn rejects_missing_name() {
    assert!(load("version = \"1.0.0\"\n").is_err());
}

#[test]
fn rejects_non_slug_name() {
    // uppercase and underscores are not allowed in a package name
    assert!(load("name = \"Acme_Web\"\nversion = \"1.0.0\"\n").is_err());
}

#[test]
fn rejects_bad_version() {
    // must be MAJOR.MINOR.PATCH
    assert!(load("name = \"pkg\"\nversion = \"1.0\"\n").is_err());
    assert!(load("name = \"pkg\"\nversion = \"latest\"\n").is_err());
}

#[test]
fn rejects_non_caret_major_dependency_constraint() {
    // Only `^MAJOR` is legal (packages.md §6): pins and ranges are invalid.
    assert!(
        load("name = \"p\"\nversion = \"1.0.0\"\n\n[dependencies]\nother = \"1.2.3\"\n").is_err()
    );
    assert!(
        load("name = \"p\"\nversion = \"1.0.0\"\n\n[dependencies]\nother = \">=1\"\n").is_err()
    );
    // the caret-major form is accepted
    assert!(load("name = \"p\"\nversion = \"1.0.0\"\n\n[dependencies]\nother = \"^2\"\n").is_ok());
}
