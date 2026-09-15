//! Embedder construction for `--embedder local|cached|openai`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vaire::VaireError;
use vaire::embed::Embedder;
use vaire::error::Result as VResult;
use vaire::userconfig::UserConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedderKind {
    Local,
    Cached,
    OpenAi,
}

impl EmbedderKind {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "local" => Ok(Self::Local),
            "cached" => Ok(Self::Cached),
            "openai" => Ok(Self::OpenAi),
            other => Err(format!(
                "unknown --embedder {other:?} (expected local|cached|openai)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Cached => "cached",
            Self::OpenAi => "openai",
        }
    }
}

/// Point `VAIRE_CONFIG_HOME` at a fresh, empty temp dir, unless `kind` is `OpenAi` — that
/// mode must see the developer's *real* config/credentials home to reach the network
/// (`UserConfig::load()` + `credentials.toml`). For `local`/`cached` this guarantees the
/// benchmark never reads (or is influenced by) the developer's real `~/.config/vaire`.
pub fn hermetic_config_home_unless_openai(kind: EmbedderKind) -> std::io::Result<()> {
    if kind == EmbedderKind::OpenAi {
        return Ok(());
    }
    let dir =
        std::env::temp_dir().join(format!("vaire-search-bench-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    // SAFETY: called once at start-of-process, before any embedder or user config is
    // constructed, and before any other thread exists (mirrors tests/common's
    // `hermetic_config_home`).
    unsafe { std::env::set_var("VAIRE_CONFIG_HOME", &dir) };
    Ok(())
}

/// A constructed embedder ready to hand to `vaire::index::build::run` / `vaire::search::search`,
/// plus (for the cache-backed modes) a way to persist newly-embedded vectors afterwards.
pub struct EmbedderHandle(Kind);

enum Kind {
    Plain(Box<dyn Embedder>),
    Cache(CacheEmbedder),
}

impl EmbedderHandle {
    /// `local`: the built-in in-process embedder, via `from_user_config(&UserConfig::default())`.
    /// `EmbeddingConfig::default()` sets `provider = Local`, so this never touches the network.
    pub fn local() -> Result<Self, String> {
        let emb = vaire::embed::from_user_config(&UserConfig::default())
            .map_err(|e| format!("building local embedder: {e}"))?;
        Ok(EmbedderHandle(Kind::Plain(emb)))
    }

    /// `cached`: vectors come only from `cache_path`; any miss is a hard error. No network,
    /// no inner embedder at all.
    pub fn cached(cache_path: &Path) -> Result<Self, String> {
        Ok(EmbedderHandle(Kind::Cache(CacheEmbedder::load_readonly(
            cache_path,
        )?)))
    }

    /// `openai`: the real network embedder via `from_user_config(&UserConfig::load()?)`,
    /// wrapped so cache hits skip the network and misses are embedded then written back to
    /// `cache_path`. Never invoked by this harness's own acceptance runs — implemented for
    /// completeness, per the issue #52 benchmark spec.
    pub fn openai(cache_path: &Path) -> Result<Self, String> {
        let user = UserConfig::load().map_err(|e| format!("loading user config: {e}"))?;
        let inner = vaire::embed::from_user_config(&user)
            .map_err(|e| format!("building openai embedder: {e}"))?;
        Ok(EmbedderHandle(Kind::Cache(CacheEmbedder::wrap(
            inner,
            cache_path.to_path_buf(),
        )?)))
    }

    pub fn as_dyn(&self) -> &dyn Embedder {
        match &self.0 {
            Kind::Plain(b) => b.as_ref(),
            Kind::Cache(c) => c,
        }
    }

    /// Write any newly-embedded vectors back to the cache file (`openai` mode only; a
    /// no-op for `local`/`cached`).
    pub fn flush(&self) -> Result<(), String> {
        match &self.0 {
            Kind::Cache(c) => c.flush(),
            Kind::Plain(_) => Ok(()),
        }
    }
}

/// On-disk shape of `--vector-cache <file>`.
#[derive(Serialize, Deserialize)]
struct CacheFile {
    identity: String,
    dims: usize,
    /// sha256 hex of the full (untruncated) text → its vector.
    vectors: HashMap<String, Vec<f32>>,
}

/// A cache is a hard limit on the request size an embedding API accepts. `--embedder
/// openai` sends miss texts as-is otherwise, so a section long enough to overflow the
/// provider's own input-size limit would fail the whole run; truncating defensively here
/// (bytes, backed off to a UTF-8 boundary) keeps the benchmark usable regardless of what a
/// given provider currently enforces. The cache *key* stays the hash of the full,
/// untruncated text, so a lookup from indexing or a later run still hits.
const MAX_EMBED_INPUT_BYTES: usize = 8000;

/// Serves vectors from a `--vector-cache` file, optionally backed by a live inner embedder
/// (`openai` mode) that fills misses and marks the cache dirty for [`CacheEmbedder::flush`].
/// `inner: None` is the `cached` mode: any miss is a hard error, never a network call.
struct CacheEmbedder {
    inner: Option<Box<dyn Embedder>>,
    path: PathBuf,
    identity: String,
    dims: usize,
    vectors: RefCell<HashMap<String, Vec<f32>>>,
    dirty: Cell<bool>,
}

impl CacheEmbedder {
    fn load_readonly(path: &Path) -> Result<Self, String> {
        let file = read_cache_file(path)?;
        Ok(CacheEmbedder {
            inner: None,
            path: path.to_path_buf(),
            identity: file.identity,
            dims: file.dims,
            vectors: RefCell::new(file.vectors),
            dirty: Cell::new(false),
        })
    }

    fn wrap(inner: Box<dyn Embedder>, path: PathBuf) -> Result<Self, String> {
        let vectors = if path.exists() {
            let file = read_cache_file(&path)?;
            if file.identity != inner.identity() {
                eprintln!(
                    "warning: --vector-cache {} was recorded under identity {:?}; the current \
                     embedder identity is {:?}. Reusing cached vectors by text hash regardless \
                     (this is a benchmarking dev tool, not a production cache).",
                    path.display(),
                    file.identity,
                    inner.identity()
                );
            }
            file.vectors
        } else {
            HashMap::new()
        };
        Ok(CacheEmbedder {
            dims: inner.dimensions(),
            identity: inner.identity(),
            inner: Some(inner),
            path,
            vectors: RefCell::new(vectors),
            dirty: Cell::new(false),
        })
    }

    fn flush(&self) -> Result<(), String> {
        if !self.dirty.get() {
            return Ok(());
        }
        let file = CacheFile {
            identity: self.identity.clone(),
            dims: self.dims,
            vectors: self.vectors.borrow().clone(),
        };
        let text = serde_json::to_string_pretty(&file)
            .map_err(|e| format!("serializing --vector-cache: {e}"))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        std::fs::write(&self.path, text)
            .map_err(|e| format!("writing --vector-cache {}: {e}", self.path.display()))?;
        self.dirty.set(false);
        Ok(())
    }
}

fn read_cache_file(path: &Path) -> Result<CacheFile, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("reading --vector-cache {}: {e}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("parsing --vector-cache {}: {e}", path.display()))
}

impl Embedder for CacheEmbedder {
    fn embed(&self, texts: &[String]) -> VResult<Vec<Vec<f32>>> {
        let mut out: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
        let mut miss_idx = Vec::new();
        {
            let map = self.vectors.borrow();
            for (i, t) in texts.iter().enumerate() {
                match map.get(&sha256_hex(t)) {
                    Some(v) => out[i] = Some(v.clone()),
                    None => miss_idx.push(i),
                }
            }
        }

        if !miss_idx.is_empty() {
            let Some(inner) = &self.inner else {
                let mut sample: Vec<&str> = miss_idx.iter().map(|&i| texts[i].as_str()).collect();
                sample.truncate(5);
                return Err(VaireError::Config(format!(
                    "cached embedder: {} of {} text(s) missing from --vector-cache {} \
                     (no network fallback); first missing: {sample:?}",
                    miss_idx.len(),
                    texts.len(),
                    self.path.display()
                )));
            };

            // Truncate each miss to at most MAX_EMBED_INPUT_BYTES bytes before sending it to
            // the inner (network) embedder, backing off to a UTF-8 char boundary. The cache
            // key below still hashes the FULL original text, so future lookups (including
            // from the index build that produced this miss) hit regardless.
            let mut truncated_count = 0usize;
            let sent: Vec<String> = miss_idx
                .iter()
                .map(|&i| {
                    let t = texts[i].as_str();
                    if t.len() > MAX_EMBED_INPUT_BYTES {
                        truncated_count += 1;
                        truncate_to_byte_boundary(t, MAX_EMBED_INPUT_BYTES)
                    } else {
                        t.to_string()
                    }
                })
                .collect();
            if truncated_count > 0 {
                eprintln!(
                    "vector-cache: truncated {truncated_count} of {} miss text(s) to \
                     {MAX_EMBED_INPUT_BYTES} bytes before embedding",
                    miss_idx.len()
                );
            }

            let embedded = inner.embed(&sent)?;
            let mut map = self.vectors.borrow_mut();
            for (&i, vector) in miss_idx.iter().zip(embedded) {
                map.insert(sha256_hex(&texts[i]), vector.clone());
                out[i] = Some(vector);
            }
            drop(map);
            self.dirty.set(true);
        }

        Ok(out
            .into_iter()
            .map(|o| o.expect("every index filled"))
            .collect())
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn identity(&self) -> String {
        self.identity.clone()
    }
}

/// Truncate `s` to at most `max_bytes` bytes, backing off to a valid UTF-8 char boundary.
fn truncate_to_byte_boundary(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        let _ = write!(s, "{b:02x}");
    }
    s
}

// See the comment on the `tests` module in `corpus.rs`: this file also compiles into the
// `harness = false` search bench, where these test-only items are unreferenced.
#[cfg(test)]
#[allow(dead_code, unused_imports)]
mod tests {
    use super::*;

    #[test]
    fn truncate_backs_off_to_a_char_boundary() {
        // "é" is 2 bytes (0xC3 0xA9); a max that lands mid-character must back off.
        let s = "aé"; // 'a' (1 byte) + 'é' (2 bytes) = 3 bytes total
        assert_eq!(truncate_to_byte_boundary(s, 2), "a");
        assert_eq!(truncate_to_byte_boundary(s, 3), "aé");
        assert_eq!(truncate_to_byte_boundary(s, 100), "aé");
    }

    #[derive(Clone)]
    struct StubEmbedder {
        dims: usize,
        calls: std::rc::Rc<std::cell::RefCell<Vec<Vec<String>>>>,
    }
    impl Embedder for StubEmbedder {
        fn embed(&self, texts: &[String]) -> VResult<Vec<Vec<f32>>> {
            self.calls.borrow_mut().push(texts.to_vec());
            Ok(texts.iter().map(|_| vec![1.0; self.dims]).collect())
        }
        fn dimensions(&self) -> usize {
            self.dims
        }
        fn identity(&self) -> String {
            "stub:1".to_string()
        }
    }

    #[test]
    fn cached_mode_errors_loudly_on_any_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("cache.json");
        let file = CacheFile {
            identity: "local:8".to_string(),
            dims: 8,
            vectors: HashMap::new(),
        };
        std::fs::write(&cache_path, serde_json::to_string(&file).unwrap()).unwrap();
        let handle = EmbedderHandle::cached(&cache_path).unwrap();
        let err = handle
            .as_dyn()
            .embed(&["missing text".to_string()])
            .unwrap_err();
        assert!(err.to_string().contains("missing from --vector-cache"));
    }

    #[test]
    fn cached_mode_serves_a_hit_with_no_inner_embedder() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("cache.json");
        let mut vectors = HashMap::new();
        vectors.insert(sha256_hex("hello"), vec![1.0, 2.0]);
        let file = CacheFile {
            identity: "local:2".to_string(),
            dims: 2,
            vectors,
        };
        std::fs::write(&cache_path, serde_json::to_string(&file).unwrap()).unwrap();
        let handle = EmbedderHandle::cached(&cache_path).unwrap();
        let out = handle.as_dyn().embed(&["hello".to_string()]).unwrap();
        assert_eq!(out, vec![vec![1.0, 2.0]]);
        assert_eq!(handle.as_dyn().identity(), "local:2");
    }

    #[test]
    fn openai_mode_truncates_the_payload_but_hashes_the_full_text_as_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("cache.json"); // does not exist yet
        let calls = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let inner = StubEmbedder {
            dims: 3,
            calls: calls.clone(),
        };
        let long_text = "x".repeat(MAX_EMBED_INPUT_BYTES + 500);
        let cache = CacheEmbedder::wrap(Box::new(inner), cache_path.clone()).unwrap();
        let out = cache.embed(std::slice::from_ref(&long_text)).unwrap();
        assert_eq!(out, vec![vec![1.0, 1.0, 1.0]]);
        // The inner embedder was called with the truncated text...
        assert_eq!(calls.borrow()[0][0].len(), MAX_EMBED_INPUT_BYTES);
        // ...but the cache key is the hash of the FULL text, so a later exact-text lookup hits.
        assert!(cache.vectors.borrow().contains_key(&sha256_hex(&long_text)));
        cache.flush().unwrap();
        assert!(cache_path.exists());
    }
}
