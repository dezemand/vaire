//! Content-hash embedding cache (design.md §9).
//!
//! "Rebuildable in seconds" only holds once embeddings exist if reindex re-embeds
//! *only changed sections*. The cache is keyed by a hash of the section text, so an
//! unchanged section hits the cache regardless of which file or commit it came from. A
//! cold rebuild re-embeds once and populates the cache.
//!
//! Lives under `.vaire/` (derived, gitignored). The simplest backing is a table in the
//! same `index.db`; this trait keeps the call site agnostic to that choice.

use crate::error::Result;

/// A stable hash of a section's text — the cache key.
pub type ContentHash = [u8; 32];

pub trait EmbedCache {
    fn get(&self, hash: &ContentHash) -> Result<Option<Vec<f32>>>;
    fn put(&mut self, hash: ContentHash, vector: &[f32]) -> Result<()>;
}

/// Hash a section body into its cache key.
///
/// Placeholder: a `DefaultHasher` digest in the low 8 bytes. Step 4 swaps in a real
/// 256-bit content hash (blake3/sha256); the 32-byte width is already the storage shape.
pub fn hash_text(text: &str) -> ContentHash {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let mut out = [0u8; 32];
    out[..8].copy_from_slice(&hasher.finish().to_le_bytes());
    out
}
