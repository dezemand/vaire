//! Moving bytes to and from a registry's base URL (registry.v2.md §8.4).
//!
//! [`StaticHttp`](super::StaticHttp) knows the *protocol* — which documents exist, what
//! they mean, what order to write them in. This module knows only how to get and put a
//! blob at a relative path. Splitting them is what makes `file://` a first-class registry
//! rather than a mock: the same protocol code runs over a directory and over a bucket, so
//! the integration suite exercises the real implementation.
//!
//! ## The two conditional writes are the whole design
//!
//! A static registry enforces its own semantics with **zero server code**, because both
//! semantics are properties of a *write* rather than of a policy check:
//!
//! * [`Transport::put_new`] — create-only (`If-None-Match: *`). A published
//!   `(name, version)` cannot be overwritten, because the storage layer itself refuses.
//!   Immutability stops being something anyone has to remember.
//! * [`Transport::put_cas`] — compare-and-swap (`If-Match: <etag>`). Two people publishing
//!   at once cannot lose each other's release: the second write is rejected, and the loser
//!   re-reads and retries (§3.4).
//!
//! ## What `file://` can and cannot promise
//!
//! `put_new` is exact: `O_CREAT|O_EXCL` is the same atomic test-and-set the object store
//! performs, delivered by the kernel.
//!
//! `put_cas` is **emulated**, and the emulation has a window. A POSIX filesystem has no
//! compare-and-swap, so the sequence is compare, then `rename` — and two processes can
//! interleave between those two steps and both believe they won. This is stated rather
//! than papered over with a lock file: a lock file would need stale-lock detection (a
//! crashed push must not wedge a registry forever), and stale-lock detection is a
//! heuristic that breaks locks it should not, which trades a microsecond window for a
//! failure mode with worse consequences. The honest position is that `file://` gives real
//! immutability and best-effort CAS, and that true CAS arrives with the object store that
//! has the primitive. Nothing in the intended use — one maintainer, or CI publishing tags
//! serially — lands inside the window.
//!
//! ## HTTP is read-only here
//!
//! [`HttpTransport`] fetches. It does not write, because writing to a bucket means the
//! provider's authentication, which is the S3 step of the build order and not this one.
//! An `https://` registry is therefore a perfectly good place to *pull* from today, and
//! `push` to one reports [`TransportError::Unsupported`] rather than pretending.

use std::io::Write;
use std::path::{Path, PathBuf};

pub type TransportResult<T> = std::result::Result<T, TransportError>;

/// How long to wait for an HTTP registry before calling it unreachable.
const HTTP_TIMEOUT_SECS: u64 = 30;

/// A document as the transport found it.
pub struct Fetched {
    pub bytes: Vec<u8>,
    /// The entity tag to hand back to [`Transport::put_cas`]. Always present for
    /// `file://` (it is derived from the content); may be absent over HTTP, where a server
    /// is free not to send one — in which case a compare-and-swap cannot be attempted and
    /// the caller must say so rather than write blind.
    pub etag: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("not found")]
    NotFound,

    /// The create-only write lost: something is already at that path. For an artifact this
    /// *is* the immutability guarantee firing.
    #[error("already exists")]
    Exists,

    /// The compare-and-swap lost: the document changed under us.
    #[error("modified by another writer")]
    Conflict,

    #[error("{0} is not supported over this transport")]
    Unsupported(&'static str),

    #[error("timed out after {0}s")]
    Timeout(u64),

    #[error("{0}")]
    Unreachable(String),

    #[error("{0}")]
    Io(String),
}

/// Get and put blobs at paths relative to a base location.
///
/// Paths are `/`-separated and always relative (`v1/index/acme-core.json`). Building them
/// is the protocol layer's job; **validating that a package name is safe to put in one is
/// too** ([`super::wire::checked_name`]), because a name reaches this layer from a manifest
/// and would otherwise be free to traverse out of the registry root.
pub trait Transport {
    fn base(&self) -> &str;

    /// The document at `path`, or `Ok(None)` if there is none.
    ///
    /// Absence is not an error: half the protocol's decisions ("has this package been
    /// published before?") are answered by a 404, and forcing every caller to pattern-match
    /// an error for the ordinary case is how a missing index document ends up reported as a
    /// broken registry.
    fn get(&self, path: &str) -> TransportResult<Option<Fetched>>;

    /// Write `bytes` at `path` **only if nothing is there** ([`TransportError::Exists`]
    /// otherwise).
    fn put_new(&self, path: &str, bytes: &[u8]) -> TransportResult<()>;

    /// Write `bytes` at `path` only if what is currently there has entity tag `etag` —
    /// or, when `etag` is `None`, only if nothing is there at all.
    ///
    /// [`TransportError::Conflict`] on a lost race.
    fn put_cas(&self, path: &str, bytes: &[u8], etag: Option<&str>) -> TransportResult<()>;

    /// Whether this transport can write at all. Read-only transports answer `false` and
    /// report [`TransportError::Unsupported`] from both writers.
    fn writable(&self) -> bool;
}

/// Open the transport for a base URL: `file://…` (read-write) or `http(s)://…`
/// (read-only). A bare filesystem path is accepted as `file://` shorthand, which is what
/// makes "publish to a directory" a one-liner.
pub fn open(url: &str) -> TransportResult<Box<dyn Transport>> {
    match scheme_of(url) {
        Some("file") => Ok(Box::new(FileTransport::from_url(url)?)),
        Some("http" | "https") => Ok(Box::new(HttpTransport::new(url))),
        Some(other) => Err(TransportError::Unsupported(match other {
            "s3" => "s3:// (use the bucket's https:// endpoint)",
            _ => "this URL scheme",
        })),
        // No scheme: a filesystem path. `vaire registry add lab ./registry` should work.
        None => Ok(Box::new(FileTransport::at(PathBuf::from(url)))),
    }
}

/// Normalize a user-supplied registry location into the URL that gets stored.
///
/// A bare path becomes an absolute `file://` URL, so the catalog never holds a location
/// whose meaning depends on the directory someone happened to run `registry add` from.
pub fn normalize_url(url: &str) -> TransportResult<String> {
    if scheme_of(url).is_some() {
        return Ok(url.trim_end_matches('/').to_string());
    }
    let path = PathBuf::from(url);
    let absolute = match path.is_absolute() {
        true => path,
        false => std::env::current_dir()
            .map_err(|e| TransportError::Io(e.to_string()))?
            .join(path),
    };
    // Canonicalize when it exists; a registry directory that does not exist yet is a
    // legitimate thing to configure (push creates it), so absence is not fatal here.
    let absolute = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    Ok(format!("file://{}", absolute.display()))
}

/// The scheme of a URL, or `None` when there is none. Deliberately stricter than "contains
/// a colon": a Windows path (`C:\reg`) has one and is not a URL.
fn scheme_of(url: &str) -> Option<&str> {
    let (scheme, _) = url.split_once("://")?;
    let ok = !scheme.is_empty()
        && scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.');
    ok.then_some(scheme)
}

// ---------------------------------------------------------------------------------------
// file://
// ---------------------------------------------------------------------------------------

/// A registry that is a directory. Fully read-write, and the substrate the conformance
/// suite runs the real protocol against.
pub struct FileTransport {
    root: PathBuf,
    base: String,
}

impl FileTransport {
    pub fn at(root: PathBuf) -> FileTransport {
        FileTransport {
            base: format!("file://{}", root.display()),
            root,
        }
    }

    pub fn from_url(url: &str) -> TransportResult<FileTransport> {
        let rest = url
            .strip_prefix("file://")
            .ok_or_else(|| TransportError::Io(format!("{url} is not a file:// URL")))?;
        // `file:///abs/path` (the canonical form) and `file://abs/path` (what people
        // actually type) both mean the same directory here. A genuine host component is
        // not something this can serve, and pretending otherwise would silently read the
        // wrong place.
        let path = match rest.strip_prefix('/') {
            Some(_) => rest.to_string(),
            None => format!("/{rest}"),
        };
        Ok(FileTransport::at(PathBuf::from(path)))
    }

    fn resolve(&self, path: &str) -> PathBuf {
        let mut resolved = self.root.clone();
        for segment in path.split('/').filter(|s| !s.is_empty()) {
            resolved.push(segment);
        }
        resolved
    }
}

impl Transport for FileTransport {
    fn base(&self) -> &str {
        &self.base
    }

    fn get(&self, path: &str) -> TransportResult<Option<Fetched>> {
        match std::fs::read(self.resolve(path)) {
            Ok(bytes) => Ok(Some(Fetched {
                etag: Some(etag_of(&bytes)),
                bytes,
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(TransportError::Io(e.to_string())),
        }
    }

    fn put_new(&self, path: &str, bytes: &[u8]) -> TransportResult<()> {
        let target = self.resolve(path);
        create_parents(&target)?;
        // The atomic test-and-set, done by the kernel. This one line is why an artifact
        // published to a directory is as immutable as one published to a bucket.
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(TransportError::Exists);
            }
            Err(e) => return Err(TransportError::Io(e.to_string())),
        };
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| TransportError::Io(e.to_string()))
    }

    fn put_cas(&self, path: &str, bytes: &[u8], etag: Option<&str>) -> TransportResult<()> {
        let target = self.resolve(path);
        create_parents(&target)?;
        // Stage first: a rename over the live document is atomic, so no reader ever sees a
        // half-written index.
        let staged = target.with_extension(format!("tmp{}", std::process::id()));
        let _cleanup = RemoveOnDrop(staged.clone());
        {
            let mut file =
                std::fs::File::create(&staged).map_err(|e| TransportError::Io(e.to_string()))?;
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|e| TransportError::Io(e.to_string()))?;
        }
        // The compare. Between here and the rename below is the documented window (module
        // docs): a filesystem has no primitive that does both at once.
        let current = self.get(path)?.and_then(|f| f.etag);
        match (etag, current.as_deref()) {
            (Some(expected), Some(found)) if expected == found => {}
            (None, None) => {}
            _ => return Err(TransportError::Conflict),
        }
        std::fs::rename(&staged, &target).map_err(|e| TransportError::Io(e.to_string()))
    }

    fn writable(&self) -> bool {
        true
    }
}

/// The entity tag of a document: a digest of its bytes.
///
/// Content-derived rather than mtime- or size-derived, matching what an object store hands
/// back, and immune to a filesystem whose timestamp resolution is coarser than the gap
/// between two writes.
fn etag_of(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("\"{:x}\"", Sha256::digest(bytes))
}

fn create_parents(target: &Path) -> TransportResult<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| TransportError::Io(e.to_string()))?;
    }
    Ok(())
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ---------------------------------------------------------------------------------------
// http(s):// — read-only
// ---------------------------------------------------------------------------------------

/// A registry served over HTTP. Reads today; writing waits for the object-store step,
/// which is where credentials belong.
pub struct HttpTransport {
    base: String,
    agent: ureq::Agent,
}

impl HttpTransport {
    pub fn new(base: &str) -> HttpTransport {
        HttpTransport {
            base: base.trim_end_matches('/').to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
                .build(),
        }
    }
}

impl Transport for HttpTransport {
    fn base(&self) -> &str {
        &self.base
    }

    fn get(&self, path: &str) -> TransportResult<Option<Fetched>> {
        let url = format!("{}/{}", self.base, path.trim_start_matches('/'));
        let response = match self.agent.get(&url).call() {
            Ok(response) => response,
            Err(ureq::Error::Status(404 | 403, _)) => return Ok(None),
            Err(ureq::Error::Status(code, _)) => {
                return Err(TransportError::Unreachable(format!(
                    "{url} answered {code}"
                )));
            }
            Err(ureq::Error::Transport(e)) if is_timeout(&e) => {
                return Err(TransportError::Timeout(HTTP_TIMEOUT_SECS));
            }
            Err(e) => return Err(TransportError::Unreachable(e.to_string())),
        };
        let etag = response.header("etag").map(str::to_string);
        let mut bytes = Vec::new();
        std::io::copy(&mut response.into_reader(), &mut bytes)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(Some(Fetched { bytes, etag }))
    }

    fn put_new(&self, _path: &str, _bytes: &[u8]) -> TransportResult<()> {
        Err(TransportError::Unsupported(
            "publishing over http (an object store needs its own credentials)",
        ))
    }

    fn put_cas(&self, _path: &str, _bytes: &[u8], _etag: Option<&str>) -> TransportResult<()> {
        Err(TransportError::Unsupported(
            "publishing over http (an object store needs its own credentials)",
        ))
    }

    fn writable(&self) -> bool {
        false
    }
}

/// Whether a `ureq` transport failure was the timeout rather than something else. Matched
/// on the message because `ureq` 2's `Transport` error does not expose its kind.
fn is_timeout(e: &ureq::Transport) -> bool {
    let text = e.to_string().to_ascii_lowercase();
    text.contains("timed out") || text.contains("timeout")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn a_published_blob_cannot_be_overwritten() {
        let dir = temp();
        let transport = FileTransport::at(dir.path().to_path_buf());
        transport
            .put_new("v1/artifacts/a/a-1.0.0.tgz", b"first")
            .unwrap();
        // The immutability guarantee, enforced by the kernel rather than by a check.
        assert!(matches!(
            transport.put_new("v1/artifacts/a/a-1.0.0.tgz", b"second"),
            Err(TransportError::Exists)
        ));
        let kept = transport
            .get("v1/artifacts/a/a-1.0.0.tgz")
            .unwrap()
            .unwrap();
        assert_eq!(kept.bytes, b"first");
    }

    #[test]
    fn compare_and_swap_rejects_the_writer_that_read_a_stale_document() {
        let dir = temp();
        let transport = FileTransport::at(dir.path().to_path_buf());
        transport.put_cas("v1/index/a.json", b"v1", None).unwrap();
        let stale = transport.get("v1/index/a.json").unwrap().unwrap();

        // Someone else publishes in between.
        let fresh = transport.get("v1/index/a.json").unwrap().unwrap();
        transport
            .put_cas("v1/index/a.json", b"v2", fresh.etag.as_deref())
            .unwrap();

        // The first writer's swap must lose — this is the push race (§3.4).
        assert!(matches!(
            transport.put_cas("v1/index/a.json", b"v1-plus-mine", stale.etag.as_deref()),
            Err(TransportError::Conflict)
        ));
        assert_eq!(
            transport.get("v1/index/a.json").unwrap().unwrap().bytes,
            b"v2"
        );
    }

    #[test]
    fn creating_a_document_that_must_not_exist_yet_is_also_a_swap() {
        let dir = temp();
        let transport = FileTransport::at(dir.path().to_path_buf());
        transport
            .put_cas("v1/index/a.json", b"first", None)
            .unwrap();
        // `None` means "expect nothing there", so a second create-shaped swap conflicts.
        assert!(matches!(
            transport.put_cas("v1/index/a.json", b"second", None),
            Err(TransportError::Conflict)
        ));
    }

    #[test]
    fn a_missing_document_is_absence_not_failure() {
        let dir = temp();
        let transport = FileTransport::at(dir.path().to_path_buf());
        assert!(transport.get("v1/index/nobody.json").unwrap().is_none());
    }

    #[test]
    fn urls_and_bare_paths_both_open() {
        let dir = temp();
        let url = normalize_url(&dir.path().display().to_string()).unwrap();
        assert!(url.starts_with("file://"));
        assert!(open(&url).unwrap().writable());
        assert!(open(&dir.path().display().to_string()).unwrap().writable());
        // Read-only, and honest about it rather than failing at publish time with an
        // unrelated message.
        assert!(!open("https://packages.example/kg").unwrap().writable());
    }

    #[test]
    fn scheme_detection_does_not_mistake_a_windows_path_for_a_url() {
        assert_eq!(scheme_of("file:///tmp/r"), Some("file"));
        assert_eq!(scheme_of("https://x.example"), Some("https"));
        assert_eq!(scheme_of(r"C:\registry"), None);
        assert_eq!(scheme_of("/srv/registry"), None);
        assert_eq!(scheme_of("./registry"), None);
    }

    #[test]
    fn an_unsupported_scheme_says_what_to_use_instead() {
        let Err(e) = open("s3://bucket/prefix") else {
            panic!("s3:// is not a transport this client speaks");
        };
        assert!(e.to_string().contains("https://"), "{e}");
    }
}
