//! Spec tests for the global user config (v0.2.0 M2).
//!
//! Machine/consumer settings — embeddings now, registry auth later — live in a per-user
//! config, not in the package manifest. Tests use the explicit-path seam (`load_from`/
//! `save_to`) so they never touch the real config home or race on env vars.

use vaire::config::EmbeddingProvider;
use vaire::userconfig::UserConfig;

#[test]
fn loads_defaults_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = UserConfig::load_from(dir.path()).unwrap();
    assert_eq!(cfg.embeddings.provider, EmbeddingProvider::Local);
    assert_eq!(cfg.embeddings.dimensions, 384);
}

#[test]
fn save_then_load_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = UserConfig::default();
    cfg.embeddings.provider = EmbeddingProvider::OpenAi;
    cfg.embeddings.embedding_model = "text-embedding-3-large".to_string();
    cfg.embeddings.dimensions = 1024;

    let path = cfg.save_to(dir.path()).unwrap();
    assert!(path.exists());
    assert_eq!(path.file_name().unwrap(), "config.toml");

    let loaded = UserConfig::load_from(dir.path()).unwrap();
    assert_eq!(loaded.embeddings.provider, EmbeddingProvider::OpenAi);
    assert_eq!(loaded.embeddings.embedding_model, "text-embedding-3-large");
    assert_eq!(loaded.embeddings.dimensions, 1024);
}

#[test]
fn save_creates_the_config_home_dir() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nested/vaire"); // does not exist yet
    UserConfig::default().save_to(&home).unwrap();
    assert!(home.join("config.toml").exists());
}

#[test]
fn credentials_save_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    vaire::userconfig::save_credential_to(dir.path(), "OPENAI_API_KEY", "sk-test").unwrap();
    assert_eq!(
        vaire::userconfig::credential_from("OPENAI_API_KEY", dir.path()).as_deref(),
        Some("sk-test")
    );
    // Absent key → None.
    assert!(vaire::userconfig::credential_from("MISSING_KEY", dir.path()).is_none());
}

#[test]
fn credentials_merge_without_clobbering() {
    let dir = tempfile::tempdir().unwrap();
    vaire::userconfig::save_credential_to(dir.path(), "OPENAI_API_KEY", "sk-a").unwrap();
    vaire::userconfig::save_credential_to(dir.path(), "OPENAI_BASE_URL", "https://x").unwrap();
    assert_eq!(
        vaire::userconfig::credential_from("OPENAI_API_KEY", dir.path()).as_deref(),
        Some("sk-a")
    );
    assert_eq!(
        vaire::userconfig::credential_from("OPENAI_BASE_URL", dir.path()).as_deref(),
        Some("https://x")
    );
}

#[cfg(unix)]
#[test]
fn credentials_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = vaire::userconfig::save_credential_to(dir.path(), "K", "v").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "credentials must be owner read/write only");
}

#[test]
fn corrupt_credentials_file_reads_as_none() {
    // A malformed credentials.toml is treated as "no credential" (with a stderr warning),
    // never a panic — resolution stays a total function.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("credentials.toml"), "not = = valid toml").unwrap();
    assert!(vaire::userconfig::credential_from("VAIRE_M2_ABSENT_KEY", dir.path()).is_none());
}

#[test]
fn env_var_wins_over_credentials_file() {
    // Unique key so no other test races on it; env is unsafe to set in edition 2024.
    let dir = tempfile::tempdir().unwrap();
    vaire::userconfig::save_credential_to(dir.path(), "VAIRE_M2_ENVWINS", "from-file").unwrap();
    unsafe { std::env::set_var("VAIRE_M2_ENVWINS", "from-env") };
    let got = vaire::userconfig::credential_from("VAIRE_M2_ENVWINS", dir.path());
    unsafe { std::env::remove_var("VAIRE_M2_ENVWINS") };
    assert_eq!(got.as_deref(), Some("from-env"));
}
