//! `vaire configure embeddings` dispatch wiring, driven through the real binary.
//!
//! The interactive `vaire configure` (no section) needs a TTY, so it is not exercised here;
//! its apply logic is the same `configure::run` covered in `tests/configure.rs`. This file
//! guards the CLI surface: the `embeddings` subcommand and the `--api-key-stdin`/`--api-url`
//! flags reach the config home via `VAIRE_CONFIG_HOME`.

use std::process::Command;

fn vaire() -> Command {
    Command::new(env!("CARGO_BIN_EXE_vaire"))
}

#[test]
fn configure_embeddings_writes_config_and_credentials() {
    let home = tempfile::tempdir().unwrap();

    let mut child = vaire()
        .env("VAIRE_CONFIG_HOME", home.path())
        // Ensure no ambient key leaks in and masks the file we assert on.
        .env_remove("OPENAI_API_KEY")
        .args([
            "configure",
            "embeddings",
            "--provider",
            "openai",
            "--model",
            "text-embedding-3-large",
            "--dimensions",
            "1024",
            "--api-key-stdin",
            "--api-url",
            "https://proxy.example/v1",
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"sk-cli-test\n")
        .unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());

    let config = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
    assert!(config.contains("provider = \"openai\""), "config: {config}");
    assert!(config.contains("dimensions = 1024"), "config: {config}");
    // The secret must never land in the committable-looking config file.
    assert!(!config.contains("sk-cli-test"), "secret leaked into config");

    let creds = std::fs::read_to_string(home.path().join("credentials.toml")).unwrap();
    assert!(creds.contains("sk-cli-test"), "creds: {creds}");
    assert!(creds.contains("https://proxy.example/v1"), "creds: {creds}");
}

#[test]
fn configure_embeddings_rejects_unknown_provider() {
    let home = tempfile::tempdir().unwrap();
    let output = vaire()
        .env("VAIRE_CONFIG_HOME", home.path())
        .args(["configure", "embeddings", "--provider", "nonsense"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "unknown provider must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("nonsense"), "stderr: {stderr}");
}

#[test]
fn malformed_credentials_file_never_echoes_the_secret() {
    // `toml`'s Display renders the offending source line. In credentials.toml that line
    // holds the API key, so an unterminated string (a plausible hand-edit) printed the
    // whole key to stderr — and stderr routinely lands in CI logs.
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("credentials.toml"),
        "OPENAI_API_KEY = \"sk-proj-SUPERSECRETVALUE123\n",
    )
    .unwrap();
    // Point the embedder at openai so resolving the credential is actually attempted.
    std::fs::write(
        home.path().join("config.toml"),
        "[embeddings]\nprovider = \"openai\"\nembedding_model = \"text-embedding-3-small\"\ndimensions = 1536\n",
    )
    .unwrap();

    let out = vaire()
        .env("VAIRE_CONFIG_HOME", home.path())
        .env_remove("OPENAI_API_KEY")
        .args(["configure", "show"])
        .output()
        .expect("vaire runs");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stderr.contains("SUPERSECRETVALUE") && !stdout.contains("SUPERSECRETVALUE"),
        "the API key leaked:\nstderr: {stderr}\nstdout: {stdout}"
    );
}
