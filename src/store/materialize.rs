//! Turning a verified artifact into a sealed store entry (store.md §5, amendment 2).
//!
//! One function, and the order of its steps is the contract:
//!
//! 1. **Verify and contain.** The digest was checked by [`crate::registry::Registry::fetch`]
//!    — that is what a [`VerifiedArtifact`] means, and why this takes one rather than a
//!    path. Containment happens here: an artifact is an archive from somewhere else, so
//!    every entry is checked before a byte is written.
//! 2. **Unpack** into a staging directory *beside* the destination, so the rename at the
//!    end cannot cross a filesystem.
//! 3. **Rebuild the index from the shipped Markdown.** The artifact's own `index.db` is
//!    read for provenance and then overwritten. This is the trust decision the whole store
//!    rests on: a published index is a file a publisher produced, and adopting it would
//!    make every consumer's answers depend on a stranger's build.
//! 4. **Re-stamp provenance** from the shipped database — `last_indexed_commit`,
//!    `index_source` — because those are facts about the release rather than claims about
//!    the graph, and losing them would make an entry unable to say which commit it is.
//! 5. **Write `source.toml`, seal, rename.** Sealing before the rename is what makes
//!    "immutable" true from the first instant the entry is visible under its real name.
//!
//! ## Concurrency
//!
//! Two processes materializing one version race safely: distinct staging directories, and
//! the loser of the rename treats "the destination is already there" as success. That is
//! sound rather than optimistic — both were unpacking the same verified bytes, so whatever
//! won is what this one would have written.
//!
//! ## Vectors
//!
//! The entry is embedded with the **consumer's** provider, because artifacts ship stripped
//! (§11) and there is nothing to adopt. When no embedder is available the entry is built
//! without vectors and search over it degrades to lexical — reported, not fatal: a package
//! you can read is worth more than a pull that refused.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::config::Config;
use crate::corpus::repo::Repo;
use crate::embed::Embedder;
use crate::error::{Result, VaireError};
use crate::index::Index;
use crate::index::build::{self, Mode};
use crate::registry::VerifiedArtifact;
use crate::store::{SOURCE_FILE, Source, Store};

/// Refuse an archive that expands past this. The artifact was verified, so this is not a
/// trust boundary so much as a blast radius: a knowledge corpus is megabytes, and anything
/// claiming to be a thousand times that is a mistake worth stopping before it fills a disk.
const MAX_UNPACKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// What materialization did.
pub struct Materialized {
    pub path: PathBuf,
    /// `false` when the entry was already there — a re-pull, or a race this process lost.
    /// Either way the store holds the right bytes.
    pub written: bool,
    pub warnings: Vec<String>,
}

/// Materialize `artifact` into `store`.
///
/// `registry` is the local name and URL it came from, recorded in `source.toml` so an entry
/// can say where it is from without the catalog.
pub fn materialize(
    store: &Store,
    artifact: &VerifiedArtifact,
    registry: Option<(&str, &str)>,
    embedder: Option<&dyn Embedder>,
) -> Result<Materialized> {
    let destination = store.entry(&artifact.name, artifact.version);
    if store.has(&artifact.name, artifact.version) {
        return Ok(Materialized {
            path: destination,
            written: false,
            warnings: Vec::new(),
        });
    }

    // Beside the destination, never in a temp dir elsewhere: the final step is a rename,
    // and a rename across filesystems is a copy that can be interrupted halfway.
    let parent = destination
        .parent()
        .ok_or_else(|| VaireError::Config(format!("{} has no parent", destination.display())))?;
    std::fs::create_dir_all(parent)?;
    // Unique per **call**, not per process: two threads materializing one version would
    // otherwise share a staging path, and each would delete the other's — which the module
    // doc above claims cannot happen.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let staging = parent.join(format!(
        ".staging-{}-{}-{}",
        std::process::id(),
        artifact.version,
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _cleanup = RemoveOnDrop(staging.clone());
    if staging.exists() {
        crate::store::unseal(&staging)?;
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;

    unpack(&artifact.path, &staging, &artifact.name, artifact.version)?;

    let manifest = staging.join("knowledge.toml");
    if !manifest.is_file() {
        return Err(VaireError::Config(format!(
            "{} {} has no knowledge.toml — it is not a package artifact",
            artifact.name, artifact.version
        )));
    }
    let config = Config::load(&manifest)?;
    if config.name != artifact.name {
        // The registry served this under one name and the manifest declares another. Which
        // one is right is not answerable from here, and installing it under either would
        // make a package resolvable by a name it does not claim.
        return Err(VaireError::Config(format!(
            "the artifact published as '{}' declares name '{}' in its manifest",
            artifact.name, config.name
        )));
    }

    // Read the shipped index for provenance *before* the rebuild overwrites it. Absent is
    // fine: a hand-assembled artifact, or one from a vaire that shipped no index.
    let shipped = staging.join(".vaire").join("index.db");
    let provenance = match shipped.is_file() {
        true => read_provenance(&shipped),
        false => BTreeMap::new(),
    };

    let mut warnings = Vec::new();
    let repo = Repo::at(staging.clone());
    match embedder {
        // `Mode::WorkingTree` rather than `Full`: the staging directory has no git, and
        // saying so explicitly beats relying on the fallback to notice.
        Some(embedder) => {
            build::run(&repo, &config, embedder, Mode::WorkingTree)?;
        }
        None => {
            build::run(&repo, &config, &NoVectors, Mode::WorkingTree)?;
            warnings.push(format!(
                "{} {} was indexed without vectors (no embedding provider configured); \
                 search over it is lexical only",
                artifact.name, artifact.version
            ));
        }
    }

    stamp(
        &Repo::index_db_at(&staging),
        &provenance,
        embedder.is_none(),
    )?;

    let source = Source {
        name: artifact.name.clone(),
        version: artifact.version,
        artifact_sha256: artifact.sha256.clone(),
        materialized_by: concat!("vaire ", env!("CARGO_PKG_VERSION")).to_string(),
        registry: registry.map(|(name, _)| name.to_string()),
        source: registry.map(|(_, url)| url.to_string()),
        materialized_at: crate::clock::now(),
    };
    std::fs::write(
        staging.join(SOURCE_FILE),
        toml::to_string_pretty(&source)
            .map_err(|e| VaireError::Config(format!("source.toml: {e}")))?,
    )?;

    // Sealed before it is visible: an entry is immutable from the first instant anything
    // can resolve to it, not from shortly afterwards.
    //
    // Everything *inside* the entry, though — not the entry directory itself. Renaming a
    // directory rewrites its `..` entry, so a read-only directory cannot be renamed, and
    // sealing the root here would make the last step fail with a permission error. The root
    // is sealed immediately after the rename instead: the window is one syscall wide, and
    // the contents are already read-only, so nothing resolvable is ever writable.
    seal_contents(&staging)?;

    match std::fs::rename(&staging, &destination) {
        Ok(()) => {
            seal_one(&destination);
            Ok(Materialized {
                path: destination,
                written: true,
                warnings,
            })
        }
        // Lost the race. Both processes unpacked the same verified bytes, so what is there
        // is what this one would have written.
        Err(_) if store.has(&artifact.name, artifact.version) => Ok(Materialized {
            path: destination,
            written: false,
            warnings,
        }),
        Err(e) => Err(e.into()),
    }
}

/// Unpack the artifact into `into`, refusing anything that is not a plain file or directory
/// inside the package's own top-level directory.
///
/// The rules, and why each one is here:
///
/// * **One top-level directory, named `<name>-<version>`**, which is stripped. An artifact
///   that spreads across several roots, or names a different package, is not the thing the
///   registry said it was serving.
/// * **No absolute paths, no `..`, no prefixes.** The classic archive escape.
/// * **Regular files and directories only.** A symlink inside an archive is an escape that
///   survives extraction; a hardlink is the same trick with a different name. Neither is
///   anything a knowledge package needs.
/// * **Under `.vaire/`, only `index.db`.** That directory is derived state this machine
///   owns — an artifact writing anything else into it would be handing a consumer's tool
///   its own configuration.
fn unpack(artifact: &Path, into: &Path, name: &str, version: crate::model::Version) -> Result<()> {
    let top = format!("{name}-{version}");
    let file = std::fs::File::open(artifact)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut unpacked: u64 = 0;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        let path = entry.path()?.into_owned();
        let refuse = |why: &str| -> VaireError {
            VaireError::Config(format!(
                "{name} {version} is not a safe artifact: {} — {why}",
                path.display()
            ))
        };

        if !(kind.is_file() || kind.is_dir()) {
            return Err(refuse(
                "only regular files and directories may appear in an artifact",
            ));
        }
        let mut components = path.components();
        match components.next() {
            Some(Component::Normal(first)) if first == top.as_str() => {}
            _ => return Err(refuse(&format!("everything must live under {top}/"))),
        }
        let relative: PathBuf = components
            .map(|component| match component {
                Component::Normal(part) => Ok(PathBuf::from(part)),
                _ => Err(refuse("paths must be relative and must not climb out")),
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .collect();
        if relative.as_os_str().is_empty() {
            continue; // the top-level directory itself
        }
        // `.vaire/` is derived state this machine owns.
        if relative.starts_with(".vaire") && relative != Path::new(".vaire/index.db") {
            return Err(refuse("only .vaire/index.db may be shipped under .vaire/"));
        }

        let target = into.join(&relative);
        if kind.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        unpacked = unpacked.saturating_add(entry.size());
        if unpacked > MAX_UNPACKED_BYTES {
            return Err(refuse("it expands to more than this tool will unpack"));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Streamed, not buffered. `entry.size()` is a number the **archive** supplies, so
        // pre-allocating it hands an attacker-controlled allocation of up to the cap above
        // — and a legitimate large attachment would cost the same memory for nothing.
        // `io::copy` keeps this flat, and drops the `as usize` truncation on 32-bit targets.
        let mut file = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut file)?;
    }
    Ok(())
}

/// The provenance keys worth carrying from the shipped index.
fn read_provenance(shipped: &Path) -> BTreeMap<String, String> {
    let mut carried = BTreeMap::new();
    let Ok(index) = Index::open(shipped) else {
        return carried;
    };
    for key in [
        "last_indexed_commit",
        "index_source",
        "deps_snapshot",
        "packed_by",
    ] {
        if let Ok(Some(value)) = index.meta(key) {
            carried.insert(key.to_string(), value);
        }
    }
    carried
}

/// Write the carried provenance onto the freshly built index, and mark what this entry is.
///
/// `index_source` is carried rather than left as the working-tree value the rebuild wrote:
/// the staging directory has no git, so the build honestly recorded "read from disk" — but
/// these files *are* that commit's tree, and the artifact said which commit. Restoring the
/// shipped answer is the accurate one, not the optimistic one.
fn stamp(
    index_db: &Path,
    provenance: &BTreeMap<String, String>,
    without_vectors: bool,
) -> Result<()> {
    let index = Index::open(index_db)?;
    for (key, value) in provenance {
        index.set_meta(key, value)?;
    }
    index.set_meta("materialized", "full")?;
    if without_vectors {
        // The build stamped whatever the no-op embedder called itself; say plainly that
        // this entry has no vectors, so a later reader does not read the identity as a
        // claim that it does.
        index.set_meta("embed_provider", "none")?;
    }
    Ok(())
}

/// `chmod a-w` the entry's contents.
///
/// Two carve-outs, and the second one is not a compromise so much as a boundary being drawn
/// in the right place:
///
/// * **`entry` itself stays writable** until after the rename — renaming a directory
///   rewrites its `..`, which a read-only directory refuses.
/// * **`.vaire/` stays writable**, because the index inside it is opened by an embedded
///   database engine that takes a write lock and creates WAL sidecars *to read*. A sealed
///   `index.db` is not an immutable index, it is an unreadable one.
///
/// What immutability is actually a claim about survives both: the **corpus** is sealed, and
/// so is `source.toml`, so "these files are release 1.4.2" cannot quietly stop being true.
/// The index is derived from those files and rebuildable from them, and nothing in this
/// crate rewrites it — the ensure pass skips store members by path, which is the behavioral
/// half this permission bit was only ever the backstop for.
fn seal_contents(entry: &Path) -> Result<()> {
    let derived = entry.join(".vaire");
    let source = entry.join(SOURCE_FILE);
    // Depth-first: a directory must not lose its write bit before the files inside it have
    // lost theirs.
    for found in walkdir::WalkDir::new(entry)
        .min_depth(1)
        .contents_first(true)
    {
        let Ok(found) = found else { continue };
        if found.path().starts_with(&derived) && found.path() != source {
            continue;
        }
        seal_one(found.path());
    }
    Ok(())
}

fn seal_one(path: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() & !0o222);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(true);
    let _ = std::fs::set_permissions(path, permissions);
}

/// An embedder that produces nothing, for the pull that has no provider configured.
///
/// Zero-dimension vectors rather than zeroed ones: a zero-length vector is what the index
/// already treats as "no embedding" (search filters on `length(vector)`), so this adds no
/// special case anywhere downstream.
struct NoVectors;

impl Embedder for NoVectors {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| Vec::new()).collect())
    }

    fn dimensions(&self) -> usize {
        0
    }

    fn identity(&self) -> String {
        "none".to_string()
    }
}

/// Remove a staging directory on every exit path. A failed materialization must not leave
/// something that looks like a half-built entry beside the real ones.
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = crate::store::unseal(&self.0);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
