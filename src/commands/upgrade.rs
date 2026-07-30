//! `vaire upgrade [<version>] [--check]` (cli.md §4.5). Maintain command — not on the
//! MCP surface, and corpus-independent: it operates on the running binary, not a
//! package, so it runs before repo discovery (like `configure`).
//!
//! Self-update follows the same contract as `install.sh`: resolve the tag from the
//! GitHub releases API (or take it pinned), download the `vaire-<tag>-<triple>`
//! archive for the compile-time target triple, extract it with the system `tar` (the
//! same tool the installer requires; bsdtar on Windows also unpacks the `.zip`
//! assets), and atomically swap it over the running executable.
//!
//! When the binary lives in a directory a package manager owns (cargo, Homebrew,
//! Nix), self-update refuses and names that manager's own upgrade command instead —
//! the seam for future package-manager distribution: those builds never self-update,
//! the manager does.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::error::{Result, VaireError};
use crate::output::UpgradeOutput;

const REPO: &str = "dezemand/vaire";
/// The release target triple this binary was built for (embedded by build.rs); the
/// release workflow publishes one asset per triple.
const TARGET: &str = env!("VAIRE_TARGET_TRIPLE");

/// Downloads larger than this abort — a vaire release binary is tens of megabytes,
/// so the cap only guards against a runaway/hostile response.
const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;
const MAX_API_RESPONSE_BYTES: u64 = 1024 * 1024;

/// Where the upgrade talks to and what it replaces. The command wires the live
/// values; tests inject a local HTTP server and a scratch "binary" to exercise the
/// whole flow (which is why the fields are public).
pub struct Source {
    /// Releases API base (live: `https://api.github.com`).
    pub api_base: String,
    /// Asset download base (live: `https://github.com`).
    pub download_base: String,
    /// The executable to replace (live: `std::env::current_exe()`).
    pub exe_path: PathBuf,
    /// Running version, no `v` prefix (live: `CARGO_PKG_VERSION`).
    pub current_version: String,
    /// Release target triple (live: the compile-time target).
    pub target: String,
}

impl Source {
    fn live() -> Result<Source> {
        Ok(Source {
            api_base: "https://api.github.com".into(),
            download_base: "https://github.com".into(),
            exe_path: std::env::current_exe()?,
            current_version: env!("CARGO_PKG_VERSION").into(),
            target: TARGET.into(),
        })
    }
}

pub fn run(version: Option<&str>, check: bool) -> Result<UpgradeOutput> {
    run_from(&Source::live()?, version, check)
}

pub fn run_from(src: &Source, version: Option<&str>, check: bool) -> Result<UpgradeOutput> {
    if let Some(hint) = managed_by(&src.exe_path) {
        return Err(VaireError::Upgrade(hint));
    }

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .user_agent(&format!("vaire/{}", src.current_version))
        .build();

    let pinned = version.is_some();
    let available = match version {
        Some(v) => normalize_version(v)?,
        None => latest_version(&agent, src)?,
    };
    // Versions are bare (`0.2.0`) everywhere the user sees them; the `v` prefix exists
    // only in the tag layer — release tags and the URLs built from them.
    let tag = format!("v{available}");

    // Unpinned runs only ever move forward, gated by a real semver comparison: install
    // only when the latest release is strictly *higher* than this build (a source build
    // ahead of the newest release must not be "upgraded" backwards). Incomparable
    // versions refuse rather than guess. An explicit version always installs (that is
    // how a corrupted install is repaired in place).
    if !pinned {
        use std::cmp::Ordering;
        let ord = compare_versions(&src.current_version, &available);
        let same =
            ord == Some(Ordering::Equal) || (ord.is_none() && src.current_version == available);
        if same || ord == Some(Ordering::Greater) {
            return Ok(UpgradeOutput {
                current: src.current_version.clone(),
                latest: available,
                target: src.target.clone(),
                up_to_date: true,
                checked_only: check,
                installed: None,
                note: (!same)
                    .then(|| "this build is newer than the latest release — nothing to do".into()),
            });
        }
        if ord.is_none() {
            return Err(VaireError::Upgrade(format!(
                "cannot compare this build ({}) with the latest release ({available}); \
                 pass the version to install explicitly: `vaire upgrade {available}`",
                src.current_version
            )));
        }
    }

    if check {
        return Ok(UpgradeOutput {
            current: src.current_version.clone(),
            latest: available,
            target: src.target.clone(),
            up_to_date: false,
            checked_only: true,
            installed: None,
            note: None,
        });
    }

    let (asset, bin_name) = asset_names(&tag, &src.target);
    let url = format!(
        "{}/{REPO}/releases/download/{tag}/{asset}",
        src.download_base
    );

    let scratch = Scratch::new()?;
    let archive = scratch.0.join(&asset);
    download(&agent, &url, &archive, &tag, &src.target)?;
    verify_checksum(&agent, src, &tag, &asset, &archive)?;
    extract(&scratch.0, &archive)?;

    // The archive holds a top-level `vaire-<tag>-<triple>/` directory with the binary;
    // fall back to a flat layout (same tolerance as install.sh).
    let stem = format!("vaire-{tag}-{}", src.target);
    let mut new_bin = scratch.0.join(&stem).join(bin_name);
    if !new_bin.is_file() {
        new_bin = scratch.0.join(bin_name);
    }
    if !new_bin.is_file() {
        return Err(VaireError::Upgrade(format!(
            "binary not found in downloaded archive {asset}"
        )));
    }

    replace_exe(&new_bin, &src.exe_path)?;

    Ok(UpgradeOutput {
        current: src.current_version.clone(),
        latest: available,
        target: src.target.clone(),
        up_to_date: false,
        checked_only: false,
        installed: Some(src.exe_path.display().to_string()),
        note: None,
    })
}

/// If the running binary lives where a package manager put it, return that manager's
/// own upgrade command. Vairë isn't shipped through any manager yet — this guard is
/// the seam for when it is.
fn managed_by(exe: &Path) -> Option<String> {
    let p = exe.to_string_lossy().replace('\\', "/");
    if p.contains("/.cargo/bin/") {
        return Some(
            "this vaire was installed with cargo; upgrade with `cargo install vaire` \
             (or `cargo install --path .` from a checkout)"
                .into(),
        );
    }
    if p.contains("/Cellar/") || p.contains("/homebrew/") || p.contains("/linuxbrew/") {
        return Some("this vaire is managed by Homebrew; upgrade with `brew upgrade vaire`".into());
    }
    if p.contains("/nix/store/") {
        return Some(
            "this vaire is managed by Nix; upgrade it through your Nix configuration".into(),
        );
    }
    None
}

/// `0.3.0` or `v0.3.0` → bare `0.3.0`. The version lands in a URL path (as the
/// `v`-prefixed tag), so only the characters release versions actually use are
/// accepted.
fn normalize_version(version: &str) -> Result<String> {
    let bare = version.strip_prefix('v').unwrap_or(version);
    if bare.is_empty()
        || !bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    {
        return Err(VaireError::Usage(format!(
            "invalid version '{version}' (expected e.g. 0.3.0)"
        )));
    }
    Ok(bare.to_string())
}

/// Both parse as `MAJOR.MINOR.PATCH` (pre-release suffix ignored) → their ordering;
/// otherwise `None` and the caller falls back to string equality.
fn compare_versions(current: &str, available: &str) -> Option<std::cmp::Ordering> {
    let triple = |v: &str| -> Option<(u64, u64, u64)> {
        let core = v.split('-').next()?;
        let mut it = core.split('.');
        let t = (
            it.next()?.parse().ok()?,
            it.next()?.parse().ok()?,
            it.next()?.parse().ok()?,
        );
        it.next().is_none().then_some(t)
    };
    Some(triple(current)?.cmp(&triple(available)?))
}

/// The latest published release's version, bare (its `tag_name` with the `v` stripped).
fn latest_version(agent: &ureq::Agent, src: &Source) -> Result<String> {
    let url = format!("{}/repos/{REPO}/releases/latest", src.api_base);
    let response = agent
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => {
                VaireError::Upgrade("no published releases found".into())
            }
            ureq::Error::Status(code, _) => {
                VaireError::Upgrade(format!("releases API returned HTTP {code} for {url}"))
            }
            other => VaireError::Upgrade(format!("could not reach {url}: {other}")),
        })?;
    let mut body = String::new();
    response
        .into_reader()
        .take(MAX_API_RESPONSE_BYTES)
        .read_to_string(&mut body)
        .map_err(|e| VaireError::Upgrade(format!("reading releases API response: {e}")))?;
    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| VaireError::Upgrade(format!("unexpected releases API response: {e}")))?;
    match json.get("tag_name").and_then(|t| t.as_str()) {
        Some(tag) => normalize_version(tag),
        None => Err(VaireError::Upgrade(
            "could not determine the latest release version (no tag_name); \
             pass one explicitly: `vaire upgrade <X.Y.Z>`"
                .into(),
        )),
    }
}

/// Release asset + binary file names for a triple (the release workflow zips Windows
/// builds and tarballs the rest).
fn asset_names(tag: &str, target: &str) -> (String, &'static str) {
    if target.contains("windows") {
        (format!("vaire-{tag}-{target}.zip"), "vaire.exe")
    } else {
        (format!("vaire-{tag}-{target}.tar.gz"), "vaire")
    }
}

fn download(agent: &ureq::Agent, url: &str, dest: &Path, tag: &str, target: &str) -> Result<()> {
    let response = agent.get(url).call().map_err(|e| match e {
        ureq::Error::Status(404, _) => VaireError::Upgrade(format!(
            "release {tag} has no prebuilt binary for {target}; \
             build from source with `cargo install --path .`"
        )),
        ureq::Error::Status(code, _) => {
            VaireError::Upgrade(format!("download failed (HTTP {code}): {url}"))
        }
        other => VaireError::Upgrade(format!("download failed: {other}")),
    })?;
    let mut file = fs::File::create(dest)?;
    let copied = std::io::copy(
        &mut response.into_reader().take(MAX_ASSET_BYTES + 1),
        &mut file,
    )
    .map_err(|e| VaireError::Upgrade(format!("download failed mid-stream: {e}")))?;
    if copied > MAX_ASSET_BYTES {
        return Err(VaireError::Upgrade(format!(
            "downloaded asset exceeds the {} MiB safety cap",
            MAX_ASSET_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}

/// Verify the downloaded archive against the release's published `SHA256SUMS`.
///
/// This runs before the archive is unpacked and swapped over the running executable.
/// HTTPS authenticates the transport, not the artifact, so on its own it is no defence
/// against a replaced release asset or a compromised publish token — and self-update is
/// the path where that matters most, since it runs unattended against whatever the
/// release currently holds.
///
/// A release with no `SHA256SUMS` (everything published before checksums existed) cannot
/// be verified, so it proceeds — matching `install.sh`. A *present* file that lacks this
/// asset, or disagrees with it, is a hard error.
///
/// That skip is a deliberate but *fail-open* concession, and worth closing once every
/// supported release publishes sums: an adversary who can replace a release asset can
/// usually also delete `SHA256SUMS`, which forces this path on an unattended self-update.
/// The fix then is to require verification above some release floor rather than infer it
/// from the file's presence.
fn verify_checksum(
    agent: &ureq::Agent,
    src: &Source,
    tag: &str,
    asset: &str,
    archive: &Path,
) -> Result<()> {
    let url = format!(
        "{}/{REPO}/releases/download/{tag}/SHA256SUMS",
        src.download_base
    );
    let sums = match agent.get(&url).call() {
        Ok(response) => response
            .into_string()
            .map_err(|e| VaireError::Upgrade(format!("could not read SHA256SUMS: {e}")))?,
        // No checksums published for this release — nothing to verify against.
        Err(ureq::Error::Status(404, _)) => return Ok(()),
        Err(ureq::Error::Status(code, _)) => {
            return Err(VaireError::Upgrade(format!(
                "could not fetch SHA256SUMS (HTTP {code}): {url}"
            )));
        }
        Err(other) => {
            return Err(VaireError::Upgrade(format!(
                "could not fetch SHA256SUMS: {other}"
            )));
        }
    };

    // Exact filename match, not a substring: a sibling asset such as `<asset>.sig` would
    // otherwise satisfy the lookup. `sha256sum` marks binary mode with a leading '*'.
    let expected = sums
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .find(|(_, name)| name.trim().trim_start_matches('*') == asset)
        .map(|(digest, _)| digest.trim().to_ascii_lowercase())
        .ok_or_else(|| {
            VaireError::Upgrade(format!(
                "release {tag} publishes SHA256SUMS but it has no entry for {asset}; \
                 refusing to install an unverifiable download"
            ))
        })?;

    let actual = sha256_file(archive)?;
    if actual != expected {
        return Err(VaireError::Upgrade(format!(
            "checksum mismatch for {asset}\n  expected {expected}\n  actual   {actual}\n\
             Refusing to install: this archive is not the one release {tag} published."
        )));
    }
    Ok(())
}

/// Lowercase hex SHA-256 of a file (shared digest helper; see `crate::hash`).
fn sha256_file(path: &Path) -> Result<String> {
    crate::hash::sha256_file(path)
        .map_err(|e| VaireError::Upgrade(format!("could not hash the download: {e}")))
}

/// Unpack with the system `tar` — the exact tool `install.sh` requires, and on
/// Windows 10+ bsdtar also reads the `.zip` assets. `-xf` auto-detects compression.
fn extract(dir: &Path, archive: &Path) -> Result<()> {
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .status()
        .map_err(|e| {
            VaireError::Upgrade(format!(
                "could not run `tar` to extract the release archive ({e}); \
                 install tar or re-run the installer script"
            ))
        })?;
    if !status.success() {
        return Err(VaireError::Upgrade(format!(
            "tar failed to extract {}",
            archive.display()
        )));
    }
    Ok(())
}

/// Swap the new binary over the running executable. The new file is first copied
/// into the executable's own directory (same filesystem), so the final step is a
/// single atomic rename — the binary is never half-written.
fn replace_exe(new_bin: &Path, exe: &Path) -> Result<()> {
    let dir = exe.parent().ok_or_else(|| {
        VaireError::Upgrade(format!("cannot resolve parent of {}", exe.display()))
    })?;
    let staged = dir.join(format!(".vaire-upgrade-{}", std::process::id()));
    let stage = |()| -> std::io::Result<()> {
        fs::copy(new_bin, &staged)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
        }
        swap(&staged, exe)
    };
    stage(()).map_err(|e| {
        // Clean up the staged copy — unless the exe slot ended up empty (Windows
        // double-failure), where staged is the only new binary left on disk.
        if exe.exists() {
            let _ = fs::remove_file(&staged);
        }
        VaireError::Upgrade(format!(
            "cannot replace {} ({e}); check write access to {}",
            exe.display(),
            dir.display()
        ))
    })
}

/// Unix: rename over the live binary — the running process keeps its open inode.
#[cfg(unix)]
fn swap(staged: &Path, exe: &Path) -> std::io::Result<()> {
    fs::rename(staged, exe)
}

/// Windows: a running .exe cannot be overwritten but can be renamed — move it aside,
/// move the new one in, and best-effort delete the old (a leftover `.old` from a
/// previous upgrade is removed by [`run_from`]'s next successful swap here).
#[cfg(windows)]
fn swap(staged: &Path, exe: &Path) -> std::io::Result<()> {
    let old = exe.with_extension("exe.old");
    let _ = fs::remove_file(&old);
    fs::rename(exe, &old)?;
    if let Err(e) = fs::rename(staged, exe) {
        // Roll the live binary back; if even that fails, say exactly where the
        // previous binary survives instead of reporting only the first error.
        if fs::rename(&old, exe).is_err() {
            return Err(std::io::Error::new(
                e.kind(),
                format!(
                    "{e}; rollback also failed — no binary is installed at {}, the \
                     previous one is preserved at {}",
                    exe.display(),
                    old.display()
                ),
            ));
        }
        return Err(e);
    }
    let _ = fs::remove_file(&old);
    Ok(())
}

/// Scratch download/extract dir under the OS temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Scratch> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("vaire-upgrade-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir)?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_pinning_normalizes_and_rejects_junk() {
        assert_eq!(normalize_version("0.3.0").unwrap(), "0.3.0");
        assert_eq!(normalize_version("v0.3.0").unwrap(), "0.3.0");
        assert_eq!(normalize_version("v1.0.0-rc.1").unwrap(), "1.0.0-rc.1");
        assert!(normalize_version("").is_err());
        assert!(normalize_version("v0.3.0/../evil").is_err());
        assert!(normalize_version("v0 .3").is_err());
    }

    #[test]
    fn version_ordering() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("0.2.0", "0.3.0"), Some(Less));
        assert_eq!(compare_versions("0.3.0", "0.3.0"), Some(Equal));
        assert_eq!(compare_versions("0.10.0", "0.9.9"), Some(Greater));
        assert_eq!(compare_versions("0.3.0-dev", "0.3.0"), Some(Equal));
        assert_eq!(compare_versions("0.3", "0.3.0"), None);
        assert_eq!(compare_versions("nightly", "0.3.0"), None);
    }

    #[test]
    fn package_manager_locations_are_guarded() {
        let managed = [
            "/Users/x/.cargo/bin/vaire",
            "/opt/homebrew/Cellar/vaire/0.2.0/bin/vaire",
            "/home/linuxbrew/.linuxbrew/bin/../Cellar/vaire/bin/vaire",
            "/nix/store/abc123-vaire-0.2.0/bin/vaire",
            r"C:\Users\x\.cargo\bin\vaire.exe",
        ];
        for p in managed {
            assert!(managed_by(Path::new(p)).is_some(), "{p} should be guarded");
        }
        assert!(managed_by(Path::new("/Users/x/.local/bin/vaire")).is_none());
        assert!(managed_by(Path::new("/usr/local/bin/vaire")).is_none());
    }

    #[test]
    fn asset_naming_matches_the_release_workflow() {
        assert_eq!(
            asset_names("v0.2.0", "aarch64-apple-darwin"),
            ("vaire-v0.2.0-aarch64-apple-darwin.tar.gz".into(), "vaire")
        );
        assert_eq!(
            asset_names("v0.2.0", "x86_64-pc-windows-msvc"),
            (
                "vaire-v0.2.0-x86_64-pc-windows-msvc.zip".into(),
                "vaire.exe"
            )
        );
    }
}
