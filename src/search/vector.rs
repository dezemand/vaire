//! Brute-force vector recall (design.md §9).
//!
//! Vectors live in the *same* `index.db` as an embedding blob column; at a few
//! thousand sections, brute-force cosine in Rust is sub-millisecond, so ANN/`sqlite-vec`
//! is unnecessary. Same reasoning as SQLite-as-graph: no separate vector store.

/// Decode a little-endian `f32` blob (as written by `index::build`) back to a vector.
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
    fn decode_roundtrips_le_f32() {
        let mut blob = Vec::new();
        for f in [0.5f32, -1.25, 3.0] {
            blob.extend_from_slice(&f.to_le_bytes());
        }
        assert_eq!(decode_vector(&blob), vec![0.5, -1.25, 3.0]);
    }
}
