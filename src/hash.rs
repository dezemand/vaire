//! Cryptographic file digests, shared by `vaire upgrade` (release verification) and
//! `vaire pack` (artifact identity). `embed::cache::hash_text` is NOT a substitute — that
//! is a DefaultHasher stub for cache keys, not a cryptographic digest (Cargo.toml note).

use std::path::Path;

use crate::error::Result;

/// Lowercase hex SHA-256 of a file, read in chunks so a large file is never fully
/// buffered in memory.
pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}
