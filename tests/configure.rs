//! Spec tests for `vaire configure` (v0.2.0 M2) — set embedding settings + secrets in the
//! global user config. Uses the explicit-home seam so it never touches the real config home.

use vaire::commands::configure::{self, ConfigureOpts};
use vaire::config::EmbeddingProvider;
use vaire::userconfig::{UserConfig, credential_from};

#[test]
fn sets_provider_model_and_dimensions() {
    let dir = tempfile::tempdir().unwrap();
    configure::run(
        dir.path(),
        ConfigureOpts {
            provider: Some("openai".into()),
            model: Some("text-embedding-3-large".into()),
            dimensions: Some(1024),
            ..Default::default()
        },
    )
    .unwrap();

    let cfg = UserConfig::load_from(dir.path()).unwrap();
    assert_eq!(cfg.embeddings.provider, EmbeddingProvider::OpenAi);
    assert_eq!(cfg.embeddings.embedding_model, "text-embedding-3-large");
    assert_eq!(cfg.embeddings.dimensions, 1024);
}

#[test]
fn stores_openai_key_in_credentials_not_config() {
    let dir = tempfile::tempdir().unwrap();
    let out = configure::run(
        dir.path(),
        ConfigureOpts {
            openai_key: Some("sk-secret".into()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(
        credential_from("OPENAI_API_KEY", dir.path()).as_deref(),
        Some("sk-secret")
    );
    assert!(out.credentials_set.contains(&"OPENAI_API_KEY".to_string()));
    // The secret must never land in the (committable-looking) config file.
    let config_text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(!config_text.contains("sk-secret"));
}

#[test]
fn partial_update_preserves_other_settings() {
    let dir = tempfile::tempdir().unwrap();
    configure::run(
        dir.path(),
        ConfigureOpts {
            dimensions: Some(512),
            ..Default::default()
        },
    )
    .unwrap();
    configure::run(
        dir.path(),
        ConfigureOpts {
            provider: Some("command".into()),
            command: Some("my-embedder".into()),
            ..Default::default()
        },
    )
    .unwrap();

    let cfg = UserConfig::load_from(dir.path()).unwrap();
    assert_eq!(cfg.embeddings.dimensions, 512); // preserved from the first call
    assert_eq!(cfg.embeddings.provider, EmbeddingProvider::Command);
    assert_eq!(cfg.embeddings.command, "my-embedder");
}

#[test]
fn rejects_unknown_provider() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        configure::run(
            dir.path(),
            ConfigureOpts {
                provider: Some("nonsense".into()),
                ..Default::default()
            },
        )
        .is_err()
    );
}
