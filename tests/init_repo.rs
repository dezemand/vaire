//! `vaire init` must honor the global `--repo` / `VAIRE_REPO` target, not only its positional
//! path. Driven through the real binary, since this is dispatch wiring in `main.rs`.

use std::process::Command;

fn vaire() -> Command {
    Command::new(env!("CARGO_BIN_EXE_vaire"))
}

#[test]
fn init_honors_repo_flag() {
    let cwd = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();

    let status = vaire()
        .current_dir(cwd.path())
        .arg("init")
        .arg("--repo")
        .arg(target.path())
        .status()
        .unwrap();

    assert!(status.success());
    assert!(
        target.path().join("knowledge.toml").exists(),
        "--repo target should get the manifest"
    );
    assert!(
        !cwd.path().join("knowledge.toml").exists(),
        "cwd must not be initialized when --repo is given"
    );
}

#[test]
fn init_honors_vaire_repo_env() {
    let cwd = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();

    let status = vaire()
        .current_dir(cwd.path())
        .env("VAIRE_REPO", target.path())
        .arg("init")
        .status()
        .unwrap();

    assert!(status.success());
    assert!(target.path().join("knowledge.toml").exists());
    assert!(!cwd.path().join("knowledge.toml").exists());
}

#[test]
fn init_positional_path_wins_over_repo() {
    let cwd = tempfile::tempdir().unwrap();
    let positional = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();

    let status = vaire()
        .current_dir(cwd.path())
        .arg("init")
        .arg(positional.path())
        .arg("--repo")
        .arg(repo.path())
        .status()
        .unwrap();

    assert!(status.success());
    assert!(
        positional.path().join("knowledge.toml").exists(),
        "an explicit positional path is the most specific target"
    );
    assert!(!repo.path().join("knowledge.toml").exists());
}
