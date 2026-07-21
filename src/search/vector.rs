//! Vector blob encode/decode + a reference cosine (design.md §9).
//!
//! Vectors live in the same `index.db` as a blob column. As of M3, similarity is computed
//! by Turso's native `vector_distance_cos` (see [`crate::search`]); this module keeps the
//! **little-endian f32 encoding** — which is byte-identical to Turso's Float32-dense layout,
//! so the same blob serves both the engine and the content-hash embedding cache — and a
//! hand-rolled [`cosine`] retained as a reference oracle for tests.

/// Encode a vector as little-endian `f32` bytes. Byte-identical to Turso's Float32-dense
/// vector layout, so the stored blob is read directly by `vector_distance_cos`.
pub fn encode_vector(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

/// Decode a little-endian `f32` blob (as written by [`encode_vector`]) back to a vector.
pub fn decode_vector(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Cosine similarity between two equal-length vectors. `0.0` for mismatched/empty/zero.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert!((cosine(&[1.0, 1.0], &[2.0, 2.0]) - 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn encode_decode_roundtrips_le_f32() {
        let v = vec![0.5f32, -1.25, 3.0];
        let blob = encode_vector(&v);
        assert_eq!(blob.len(), v.len() * 4);
        assert_eq!(decode_vector(&blob), v);
    }
}
