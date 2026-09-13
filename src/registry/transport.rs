//! Moving bytes to and from a registry's base URL (registry.md §9.2).
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
//! ## HTTP writes ride the same two conditional headers
//!
//! [`HttpTransport`] performs the same create-only / compare-and-swap writes as
//! `file://`, over the `If-None-Match: *` / `If-Match: <etag>` headers S3 (and any
//! WebDAV-shaped store) already understands. No credentials are attached here — an
//! authenticated bucket is `writable: true` at this layer and refuses at the HTTP layer
//! with whatever the server says, which surfaces as an ordinary
//! [`TransportError::Unreachable`] rather than a silent `Unsupported`.

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
        // half-written index. Unique **per call**, not per process — two threads swapping
        // one document would otherwise share a staging path, and one `RemoveOnDrop` would
        // delete the file the other is about to rename.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let staged = target.with_extension(format!("tmp{}.{seq}", std::process::id()));
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
// http(s):// — reads always; writes when the server honors conditional PUT
// ---------------------------------------------------------------------------------------

/// A registry served over HTTP(S).
///
/// Writes ride the same two conditional headers S3 (and any WebDAV-shaped store) already
/// understands: `If-None-Match: *` for create-only, `If-Match: <etag>` for compare-and-swap
/// (module docs, and registry.md §9.2). No credentials are attached here — an authenticated
/// bucket is `writable: true` at the transport layer and refuses at the HTTP layer with
/// whatever the server says, which surfaces as an ordinary [`TransportError::Unreachable`]
/// rather than a silent `Unsupported`. Bearer-token auth, if a deployment needs it, is a
/// header added at construction, not a reason to route writes through a different path.
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

    fn url_for(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    /// `PUT bytes` at `path` with one conditional header, translating the server's answer
    /// into the shared [`TransportError`] vocabulary.
    ///
    /// A 412 (Precondition Failed) is what both S3 and a compliant `If-Match`/`If-None-Match`
    /// server return for a lost condition; a 409 (Conflict) is folded in alongside it because
    /// some object stores answer a lost create-only write that way instead. Which *meaning*
    /// a lost condition has — "already exists" vs "changed under us" — is not recoverable
    /// from the status code alone, so the caller passes down which one applies: [`put_new`]
    /// and [`put_cas`] each know which condition they sent.
    ///
    /// [`put_new`]: Transport::put_new
    /// [`put_cas`]: Transport::put_cas
    fn conditional_put(
        &self,
        path: &str,
        bytes: &[u8],
        header: &str,
        value: &str,
        on_lost_condition: TransportError,
    ) -> TransportResult<()> {
        let url = self.url_for(path);
        match self.agent.put(&url).set(header, value).send_bytes(bytes) {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(412 | 409, _)) => Err(on_lost_condition),
            Err(ureq::Error::Transport(e)) if is_timeout(&e) => {
                Err(TransportError::Timeout(HTTP_TIMEOUT_SECS))
            }
            Err(e) => Err(TransportError::Unreachable(e.to_string())),
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
            Err(ureq::Error::Status(404, _)) => return Ok(None),
            // Not folded into absence, though it is tempting: a bucket whose objects are
            // private answers 403 for the descriptor, and reading that as "there is no
            // registry here" points the user at the wrong problem entirely. The registry
            // may well be there; this client may not read it.
            Err(ureq::Error::Status(403, _)) => {
                return Err(TransportError::Unreachable(format!(
                    "{url} answered 403 — something is there, but this client may not read it"
                )));
            }
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

    fn put_new(&self, path: &str, bytes: &[u8]) -> TransportResult<()> {
        self.conditional_put(path, bytes, "If-None-Match", "*", TransportError::Exists)
    }

    fn put_cas(&self, path: &str, bytes: &[u8], etag: Option<&str>) -> TransportResult<()> {
        match etag {
            Some(etag) => {
                self.conditional_put(path, bytes, "If-Match", etag, TransportError::Conflict)
            }
            // `None` means "expect nothing there" — the same create-shaped swap `file://`
            // treats as a conflict on a second call (module tests), so the two transports
            // agree on what a `None` etag means.
            None => {
                self.conditional_put(path, bytes, "If-None-Match", "*", TransportError::Conflict)
            }
        }
    }

    /// Optimistic: a transport with no credentials attached will find out it cannot write
    /// the moment it tries, which surfaces as an ordinary [`TransportError::Unreachable`] —
    /// not as this method lying about a capability it cannot actually check from a URL alone.
    fn writable(&self) -> bool {
        true
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
        // `writable()` is now optimistic for http(s): a URL alone cannot say whether the
        // server behind it accepts conditional writes, so the transport claims capability
        // and lets the first PUT find out for real (see the http conditional-write tests
        // below).
        assert!(open("https://packages.example/kg").unwrap().writable());
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

    // -------------------------------------------------------------------------------
    // HttpTransport conditional writes, against a minimal server this test owns.
    //
    // There is no HTTP-mock crate in dev-dependencies, so `ConditionalPutServer` below
    // implements just enough of S3's conditional-PUT contract (If-None-Match: *,
    // If-Match: <etag>, both answered with 412 on a lost condition) to prove
    // `HttpTransport` speaks it correctly. It is deliberately not a general HTTP
    // server: one request parsed at a time, on one thread, is exactly the shape a
    // synchronous `Transport` call makes.
    // -------------------------------------------------------------------------------

    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Read};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    struct ConditionalPutServer {
        base: String,
        store: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    }

    impl ConditionalPutServer {
        /// Start the server and serve exactly `requests` HTTP requests on a background
        /// thread, then stop. Callers must send exactly that many — it is what lets the
        /// thread exit instead of blocking on `accept` forever after the test returns.
        fn start(requests: usize) -> ConditionalPutServer {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let store: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
            let store_for_thread = store.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming().take(requests) {
                    let stream = stream.unwrap();
                    handle_one(stream, &store_for_thread);
                }
            });
            ConditionalPutServer { base, store }
        }

        fn etag_of(&self, path: &str) -> Option<String> {
            self.store
                .lock()
                .unwrap()
                .get(path)
                .map(|bytes| etag_of(bytes))
        }
    }

    fn handle_one(mut stream: TcpStream, store: &Arc<Mutex<HashMap<String, Vec<u8>>>>) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts
            .next()
            .unwrap_or("/")
            .trim_start_matches('/')
            .to_string();

        let mut content_length = 0usize;
        let mut if_none_match = None;
        let mut if_match = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => content_length = value.trim().parse().unwrap_or(0),
                    "if-none-match" => if_none_match = Some(value.trim().to_string()),
                    "if-match" => if_match = Some(value.trim().to_string()),
                    _ => {}
                }
            }
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            reader.read_exact(&mut body).unwrap();
        }

        let response = match method.as_str() {
            "GET" => {
                let store = store.lock().unwrap();
                match store.get(&path) {
                    Some(bytes) => http_ok_with_etag(bytes, &etag_of(bytes)),
                    None => http_status(404, b""),
                }
            }
            "PUT" => {
                let mut store = store.lock().unwrap();
                let current = store.get(&path).map(|b| etag_of(b));
                let precondition_ok = match (&if_none_match, &if_match) {
                    (Some(v), _) if v == "*" => current.is_none(),
                    (_, Some(expected)) => current.as_deref() == Some(expected.as_str()),
                    _ => true,
                };
                if precondition_ok {
                    store.insert(path.clone(), body);
                    http_status(200, b"")
                } else {
                    http_status(412, b"")
                }
            }
            _ => http_status(405, b""),
        };
        stream.write_all(&response).unwrap();
    }

    fn http_status(code: u16, body: &[u8]) -> Vec<u8> {
        let reason = match code {
            200 => "OK",
            404 => "Not Found",
            405 => "Method Not Allowed",
            412 => "Precondition Failed",
            _ => "Error",
        };
        let mut out = format!(
            "HTTP/1.1 {code} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn http_ok_with_etag(body: &[u8], etag: &str) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: {etag}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn http_create_only_write_is_rejected_the_second_time() {
        let server = ConditionalPutServer::start(3);
        let transport = HttpTransport::new(&server.base);
        transport
            .put_new("v1/artifacts/a/a-1.0.0.tgz", b"first")
            .unwrap();
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
    fn http_compare_and_swap_rejects_a_stale_writer() {
        let server = ConditionalPutServer::start(4);
        let transport = HttpTransport::new(&server.base);
        transport.put_cas("v1/index/a.json", b"v1", None).unwrap();
        let stale_etag = server.etag_of("v1/index/a.json");

        transport
            .put_cas("v1/index/a.json", b"v2", stale_etag.as_deref())
            .unwrap();

        // The same (now stale) etag must not swap again — the push race, over HTTP.
        assert!(matches!(
            transport.put_cas("v1/index/a.json", b"v1-plus-mine", stale_etag.as_deref()),
            Err(TransportError::Conflict)
        ));
        assert_eq!(
            transport.get("v1/index/a.json").unwrap().unwrap().bytes,
            b"v2"
        );
    }

    #[test]
    fn http_writable_is_optimistic_until_a_write_is_attempted() {
        let server = ConditionalPutServer::start(1);
        let transport = HttpTransport::new(&server.base);
        assert!(transport.writable());
        // The actual proof: a write against this real (test) server succeeds.
        transport.put_new("v1/index/a.json", b"{}").unwrap();
    }
}
