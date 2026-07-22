//! Behavior tests for `vaire upgrade` — the whole flow (resolve → download → extract
//! → swap) runs against a local HTTP server standing in for the GitHub releases API
//! and download host, replacing a scratch file standing in for the installed binary.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;

use vaire::commands::upgrade::{self, Source};
use vaire::error::VaireError;

const TRIPLE: &str = "aarch64-apple-darwin";

/// A one-thread HTTP/1.1 server serving canned responses for the two endpoints the
/// upgrade flow hits. Runs until the listener is dropped with the test.
struct MockGithub {
    base: String,
    _thread: std::thread::JoinHandle<()>,
}

impl MockGithub {
    /// `latest_tag`: what `/releases/latest` reports. `asset`: `(name, bytes)` served
    /// under `/dezemand/vaire/releases/download/<tag>/<name>`; anything else is 404.
    fn serve(latest_tag: &str, asset: Option<(String, Vec<u8>)>) -> MockGithub {
        // No SHA256SUMS: the release predates checksums, so verification is skipped.
        MockGithub::serve_with_sums(latest_tag, asset, None)
    }

    /// As [`MockGithub::serve`], plus a `SHA256SUMS` body served under the release tag.
    fn serve_with_sums(
        latest_tag: &str,
        asset: Option<(String, Vec<u8>)>,
        sums: Option<String>,
    ) -> MockGithub {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let latest = format!(r#"{{"tag_name": "{latest_tag}", "name": "irrelevant"}}"#);
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();

                let (status, body): (&str, Vec<u8>) =
                    if path == "/repos/dezemand/vaire/releases/latest" {
                        ("200 OK", latest.clone().into_bytes())
                    } else if path.ends_with("/SHA256SUMS") {
                        match &sums {
                            Some(body) => ("200 OK", body.clone().into_bytes()),
                            None => ("404 Not Found", b"not found".to_vec()),
                        }
                    } else if let Some((name, bytes)) = asset
                        .as_ref()
                        .filter(|(name, _)| path.ends_with(&format!("/{name}")))
                    {
                        let _ = name;
                        ("200 OK", bytes.clone())
                    } else {
                        ("404 Not Found", b"not found".to_vec())
                    };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        MockGithub {
            base,
            _thread: thread,
        }
    }
}

/// A real `vaire-<tag>-<triple>.tar.gz` in the release layout: a top-level stem
/// directory holding a `vaire` file with `content` as its bytes.
fn release_tarball(tag: &str, content: &str) -> (String, Vec<u8>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let stem = format!("vaire-{tag}-{TRIPLE}");
    let name = format!("{stem}.tar.gz");
    std::fs::create_dir(dir.path().join(&stem)).unwrap();
    std::fs::write(dir.path().join(&stem).join("vaire"), content).unwrap();
    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&name)
        .arg(&stem)
        .current_dir(dir.path())
        .status()
        .expect("tar available");
    assert!(status.success());
    let bytes = std::fs::read(dir.path().join(&name)).unwrap();
    (name, bytes)
}

/// A Source pointing every endpoint at the mock and the "installed binary" at a
/// scratch file containing `old-binary`.
fn source(mock: &MockGithub, exe_dir: &Path, current: &str) -> Source {
    let exe = exe_dir.join("vaire");
    std::fs::write(&exe, "old-binary").unwrap();
    Source {
        api_base: mock.base.clone(),
        download_base: mock.base.clone(),
        exe_path: exe,
        current_version: current.into(),
        target: TRIPLE.into(),
    }
}

#[test]
fn upgrades_to_the_latest_release_by_replacing_the_binary() {
    let (name, bytes) = release_tarball("v0.9.0", "new-binary v0.9.0");
    let mock = MockGithub::serve("v0.9.0", Some((name, bytes)));
    let dir = tempfile::tempdir().unwrap();
    let src = source(&mock, dir.path(), "0.2.0");

    let out = upgrade::run_from(&src, None, false).expect("upgrade succeeds");

    assert_eq!(out.latest, "0.9.0");
    assert!(!out.up_to_date);
    assert_eq!(
        out.installed.as_deref(),
        Some(src.exe_path.to_str().unwrap())
    );
    let installed = std::fs::read_to_string(&src.exe_path).unwrap();
    assert_eq!(installed, "new-binary v0.9.0");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&src.exe_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "installed binary must be executable");
    }
}

#[test]
fn up_to_date_makes_no_network_download_and_touches_nothing() {
    // No asset is served: reaching the download path would 404 and fail the test.
    let mock = MockGithub::serve("v0.2.0", None);
    let dir = tempfile::tempdir().unwrap();
    let src = source(&mock, dir.path(), "0.2.0");

    let out = upgrade::run_from(&src, None, false).expect("no-op succeeds");

    assert!(out.up_to_date);
    assert!(out.installed.is_none());
    assert_eq!(
        std::fs::read_to_string(&src.exe_path).unwrap(),
        "old-binary"
    );
}

#[test]
fn a_source_build_ahead_of_the_latest_release_is_not_downgraded() {
    let (name, bytes) = release_tarball("v0.1.0", "old release");
    let mock = MockGithub::serve("v0.1.0", Some((name, bytes)));
    let dir = tempfile::tempdir().unwrap();
    let src = source(&mock, dir.path(), "0.2.0");

    let out = upgrade::run_from(&src, None, false).expect("no-op succeeds");

    assert!(out.up_to_date);
    assert!(out.note.is_some(), "explains why nothing happened");
    assert_eq!(
        std::fs::read_to_string(&src.exe_path).unwrap(),
        "old-binary"
    );
}

#[test]
fn check_reports_the_available_upgrade_without_installing() {
    let (name, bytes) = release_tarball("v0.9.0", "new-binary");
    let mock = MockGithub::serve("v0.9.0", Some((name, bytes)));
    let dir = tempfile::tempdir().unwrap();
    let src = source(&mock, dir.path(), "0.2.0");

    let out = upgrade::run_from(&src, None, true).expect("check succeeds");

    assert!(out.checked_only);
    assert!(!out.up_to_date);
    assert_eq!(out.latest, "0.9.0");
    assert!(out.installed.is_none());
    assert_eq!(
        std::fs::read_to_string(&src.exe_path).unwrap(),
        "old-binary"
    );
}

#[test]
fn a_pinned_version_installs_even_when_it_matches_the_current_one() {
    // Repair flow: `vaire upgrade v0.2.0` while running 0.2.0 reinstalls in place
    // (and never consults /releases/latest — the mock reports a bogus newer tag).
    let (name, bytes) = release_tarball("v0.2.0", "reinstalled v0.2.0");
    let mock = MockGithub::serve("v9.9.9", Some((name, bytes)));
    let dir = tempfile::tempdir().unwrap();
    let src = source(&mock, dir.path(), "0.2.0");

    let out = upgrade::run_from(&src, Some("0.2.0"), false).expect("pinned install succeeds");

    assert_eq!(out.latest, "0.2.0");
    assert_eq!(
        std::fs::read_to_string(&src.exe_path).unwrap(),
        "reinstalled v0.2.0"
    );
}

#[test]
fn a_missing_platform_asset_names_the_target_and_suggests_source_build() {
    let mock = MockGithub::serve("v0.9.0", None); // release exists, asset doesn't
    let dir = tempfile::tempdir().unwrap();
    let src = source(&mock, dir.path(), "0.2.0");

    let err = upgrade::run_from(&src, None, false).expect_err("404 asset fails");

    match err {
        VaireError::Upgrade(msg) => {
            assert!(msg.contains(TRIPLE), "names the target triple: {msg}");
            assert!(
                msg.contains("cargo install"),
                "suggests source build: {msg}"
            );
        }
        other => panic!("expected upgrade error, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&src.exe_path).unwrap(),
        "old-binary"
    );
}

#[test]
fn an_incomparable_latest_version_refuses_and_asks_for_an_explicit_pin() {
    // The semver gate must not guess: a latest release that doesn't parse as
    // MAJOR.MINOR.PATCH is neither installed nor ignored — it errors with the pin hint.
    let mock = MockGithub::serve("nightly", None);
    let dir = tempfile::tempdir().unwrap();
    let src = source(&mock, dir.path(), "0.2.0");

    let err = upgrade::run_from(&src, None, false).expect_err("incomparable refuses");

    match err {
        VaireError::Upgrade(msg) => {
            assert!(msg.contains("nightly"), "names the release: {msg}");
            assert!(msg.contains("vaire upgrade"), "suggests pinning: {msg}");
        }
        other => panic!("expected upgrade error, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&src.exe_path).unwrap(),
        "old-binary"
    );
}

#[test]
fn a_package_managed_binary_refuses_and_names_the_manager() {
    let mock = MockGithub::serve("v0.9.0", None);
    let dir = tempfile::tempdir().unwrap();
    let managed = dir.path().join(".cargo").join("bin");
    std::fs::create_dir_all(&managed).unwrap();
    let mut src = source(&mock, &managed, "0.2.0");
    src.exe_path = managed.join("vaire");

    let err = upgrade::run_from(&src, None, false).expect_err("managed binary refuses");

    match err {
        VaireError::Upgrade(msg) => assert!(msg.contains("cargo install"), "{msg}"),
        other => panic!("expected upgrade error, got {other:?}"),
    }
}

// ---- release-download verification ----------------------------------------

/// Lowercase hex SHA-256, computed the way `sha256sum` prints it.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn a_matching_checksum_lets_the_upgrade_proceed() {
    let (name, bytes) = release_tarball("v0.9.0", "new-binary v0.9.0");
    let sums = format!("{}  {name}\n", sha256_hex(&bytes));
    let mock = MockGithub::serve_with_sums("v0.9.0", Some((name, bytes)), Some(sums));
    let dir = tempfile::tempdir().unwrap();
    let src = source(&mock, dir.path(), "0.2.0");

    let out = upgrade::run_from(&src, None, false).expect("upgrade succeeds");
    assert_eq!(out.latest, "0.9.0");
    assert_eq!(
        std::fs::read_to_string(&src.exe_path).unwrap(),
        "new-binary v0.9.0"
    );
}

#[test]
fn a_tampered_archive_is_refused_and_the_binary_is_left_alone() {
    // The published digest is for the genuine archive; the server hands back a different
    // one — a replaced release asset or a compromised publish token. Self-update runs
    // unattended, so this must fail closed rather than swap in whatever was served.
    let (name, genuine) = release_tarball("v0.9.0", "genuine v0.9.0");
    let (_, trojaned) = release_tarball("v0.9.0", "TROJANED PAYLOAD");
    let sums = format!("{}  {name}\n", sha256_hex(&genuine));
    let mock = MockGithub::serve_with_sums("v0.9.0", Some((name, trojaned)), Some(sums));
    let dir = tempfile::tempdir().unwrap();
    let src = source(&mock, dir.path(), "0.2.0");

    let err = upgrade::run_from(&src, None, false).expect_err("must refuse");
    let msg = err.to_string();
    assert!(
        msg.contains("checksum mismatch"),
        "expected a checksum mismatch, got: {msg}"
    );
    // The running executable is untouched.
    assert_eq!(
        std::fs::read_to_string(&src.exe_path).unwrap(),
        "old-binary",
        "a refused upgrade must not replace the binary"
    );
}

#[test]
fn published_checksums_without_an_entry_for_this_asset_are_refused() {
    // SHA256SUMS exists but says nothing about our asset — it cannot be verified, and a
    // silently-unverified install is exactly what the file is meant to prevent.
    let (name, bytes) = release_tarball("v0.9.0", "new-binary v0.9.0");
    let sums = format!("{}  some-other-asset.tar.gz\n", sha256_hex(&bytes));
    let mock = MockGithub::serve_with_sums("v0.9.0", Some((name, bytes)), Some(sums));
    let dir = tempfile::tempdir().unwrap();
    let src = source(&mock, dir.path(), "0.2.0");

    let err = upgrade::run_from(&src, None, false).expect_err("must refuse");
    assert!(err.to_string().contains("no entry for"), "got: {err}");
    assert_eq!(
        std::fs::read_to_string(&src.exe_path).unwrap(),
        "old-binary"
    );
}
