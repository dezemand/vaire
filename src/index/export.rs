//! Exporting an index for an artifact — `vaire pack`'s half of "the index is the
//! manifest" (registry.md §5.1).
//!
//! The packed `.vaire/index.db` is not a byte-copy of the working index: that file's
//! physical layout depends on its build history, it carries the machine-local
//! `embed_cache`, and its `deps_snapshot` records absolute roots. The export is a
//! **fresh database written in a fixed order** — same logical content, deterministic
//! bytes, nothing machine-local:
//!
//! - every graph table copied in a stable `ORDER BY` (nodes, node_files, edges,
//!   unresolved, sections, and — unless stripped — embeddings);
//! - the FTS structure does NOT ship (its segments embed random identity); it is
//!   recreated when the artifact is materialized into a store, before the entry
//!   becomes immutable;
//! - `embed_cache` never ships (a build-time cache, meaningless off this machine);
//! - `deps_snapshot` is rewritten to `{name, version, constraint}` — an artifact records
//!   choices, never locations (registry.md §5.1);
//! - provenance meta (`last_indexed_commit`, `index_source`, `package_name`,
//!   `embed_provider`) is carried over, plus `packed_by` (the packing vaire's version).

use std::path::Path;

use turso::Value;

use crate::error::{Result, VaireError};
use crate::index::db::{Index, SCHEMA_VERSION, col_blob, col_opt_text, col_text, col_u32};

/// What the export wrote — surfaced in `vaire pack`'s output.
#[derive(Debug, Clone, Copy)]
pub struct ExportStats {
    pub nodes: usize,
    pub edges: usize,
    pub sections: usize,
    pub embeddings: usize,
}

/// Write the artifact form of the index at `src` to a fresh database at `dest`
/// (overwriting it). The source must be a current-schema index — `vaire pack` builds it
/// immediately beforehand, so a mismatch means the build itself is broken.
pub(crate) fn export_artifact_index(
    src_path: &Path,
    dest_path: &Path,
    include_embeddings: bool,
    packed_by: &str,
) -> Result<ExportStats> {
    let src = Index::open(src_path)?;
    if src.schema_version() != Some(SCHEMA_VERSION) {
        return Err(VaireError::IndexCorrupt(format!(
            "cannot export an index with schema version {:?} (this vaire writes {SCHEMA_VERSION}); \
             rebuild with `vaire index --full`",
            src.schema_version()
        )));
    }

    crate::index::build::remove_db_files(dest_path)?;
    let dest = Index::create_for_bulk_load(dest_path)?;

    let stats = dest.with_tx(|dest| {
        let nodes = copy_nodes(&src, dest)?;
        copy_node_files(&src, dest)?;
        let edges = copy_edges(&src, dest)?;
        copy_unresolved(&src, dest)?;
        let sections = copy_sections(&src, dest)?;
        let embeddings = if include_embeddings {
            copy_embeddings(&src, dest)?
        } else {
            0
        };
        copy_meta(&src, dest, packed_by)?;
        Ok(ExportStats {
            nodes,
            edges,
            sections,
            embeddings,
        })
    })?;

    // Deliberately NO `ensure_fts_index` here: the native FTS structure embeds random
    // segment identity, which would make the artifact non-reproducible for nothing — it
    // is derived state over `sections`, rebuildable at any time. Whoever materializes
    // the artifact into a store creates it then (the same create-after-bulk-load pattern
    // the index builder uses), before the entry becomes immutable.

    // Fold the WAL into the main file — Turso does NOT checkpoint on close (the index
    // builder's promote step moves the sidecars along for the same reason). The artifact
    // is one file; content left in `-wal` would silently ship an incomplete index.
    dest.query_rows("PRAGMA wal_checkpoint(TRUNCATE)", (), |_| Ok(()))?;

    // Close both handles, then verify the artifact index actually opens and holds what
    // was written — this file ships; a torn export must fail here, not at a consumer.
    drop(dest);
    drop(src);
    let check = Index::open(dest_path)?;
    if check.schema_version() != Some(SCHEMA_VERSION) {
        return Err(VaireError::IndexCorrupt(
            "exported artifact index has no readable schema version".into(),
        ));
    }
    let count = check.scalar_i64("SELECT count(*) FROM nodes", ())? as usize;
    if count != stats.nodes {
        return Err(VaireError::IndexCorrupt(format!(
            "exported artifact index holds {count} nodes, expected {}",
            stats.nodes
        )));
    }
    drop(check);

    // The explicit checkpoint above is what folded the WAL — closing does NOT (the
    // index builder's promote step moves the sidecars along for the same reason). A
    // leftover non-empty sidecar means the checkpoint did not do its job and part of
    // the artifact's content lives outside the one file we ship — refuse to continue.
    for suffix in ["-wal", "-shm"] {
        let sidecar = std::path::PathBuf::from(format!("{}{suffix}", dest_path.display()));
        match std::fs::metadata(&sidecar) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
            Ok(meta) if suffix == "-wal" && meta.len() > 0 => {
                return Err(VaireError::IndexCorrupt(
                    "artifact index export left an unfolded WAL; refusing to ship a partial index"
                        .into(),
                ));
            }
            Ok(_) => std::fs::remove_file(&sidecar)?,
        }
    }

    Ok(stats)
}

fn opt(value: Option<String>) -> Value {
    value.map(Value::Text).unwrap_or(Value::Null)
}

fn copy_nodes(src: &Index, dest: &Index) -> Result<usize> {
    let rows = src.query_rows(
        "SELECT id, type, path, frontmatter, superseded_by, package, alias_text
         FROM nodes ORDER BY id",
        (),
        |r| {
            Ok((
                col_text(r, 0)?,
                col_text(r, 1)?,
                col_text(r, 2)?,
                col_text(r, 3)?,
                col_opt_text(r, 4)?,
                col_text(r, 5)?,
                col_text(r, 6)?,
            ))
        },
    )?;
    let n = rows.len();
    for (id, ty, path, fm, superseded, package, alias_text) in rows {
        dest.execute(
            "INSERT INTO nodes(id, type, path, frontmatter, superseded_by, package, alias_text)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            turso::params![
                Value::Text(id),
                Value::Text(ty),
                Value::Text(path),
                Value::Text(fm),
                opt(superseded),
                Value::Text(package),
                Value::Text(alias_text)
            ],
        )?;
    }
    Ok(n)
}

fn copy_node_files(src: &Index, dest: &Index) -> Result<usize> {
    let rows = src.query_rows(
        "SELECT id, path FROM node_files ORDER BY id, path",
        (),
        |r| Ok((col_text(r, 0)?, col_text(r, 1)?)),
    )?;
    let n = rows.len();
    for (id, path) in rows {
        dest.execute(
            "INSERT INTO node_files(id, path) VALUES(?1, ?2)",
            turso::params![Value::Text(id), Value::Text(path)],
        )?;
    }
    Ok(n)
}

fn copy_edges(src: &Index, dest: &Index) -> Result<usize> {
    let rows = src.query_rows(
        "SELECT from_id, to_id, to_package, ref_type, source_file, line
         FROM edges ORDER BY source_file, line, from_id, to_id, ref_type",
        (),
        |r| {
            Ok((
                col_text(r, 0)?,
                col_text(r, 1)?,
                col_opt_text(r, 2)?,
                col_text(r, 3)?,
                col_text(r, 4)?,
                col_u32(r, 5)?,
            ))
        },
    )?;
    let n = rows.len();
    for (from, to, to_package, ref_type, source_file, line) in rows {
        dest.execute(
            "INSERT INTO edges(from_id, to_id, to_package, ref_type, source_file, line)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            turso::params![
                Value::Text(from),
                Value::Text(to),
                opt(to_package),
                Value::Text(ref_type),
                Value::Text(source_file),
                Value::Integer(i64::from(line))
            ],
        )?;
    }
    Ok(n)
}

fn copy_unresolved(src: &Index, dest: &Index) -> Result<usize> {
    let rows = src.query_rows(
        "SELECT record_id, type_guess, descriptor, source_file, line
         FROM unresolved ORDER BY source_file, line, descriptor",
        (),
        |r| {
            Ok((
                col_text(r, 0)?,
                col_opt_text(r, 1)?,
                col_text(r, 2)?,
                col_text(r, 3)?,
                col_u32(r, 4)?,
            ))
        },
    )?;
    let n = rows.len();
    for (record_id, type_guess, descriptor, source_file, line) in rows {
        dest.execute(
            "INSERT INTO unresolved(record_id, type_guess, descriptor, source_file, line)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            turso::params![
                Value::Text(record_id),
                opt(type_guess),
                Value::Text(descriptor),
                Value::Text(source_file),
                Value::Integer(i64::from(line))
            ],
        )?;
    }
    Ok(n)
}

fn copy_sections(src: &Index, dest: &Index) -> Result<usize> {
    let rows = src.query_rows(
        "SELECT node_id, heading, line, body FROM sections ORDER BY node_id, line",
        (),
        |r| {
            Ok((
                col_text(r, 0)?,
                col_text(r, 1)?,
                col_u32(r, 2)?,
                col_text(r, 3)?,
            ))
        },
    )?;
    let n = rows.len();
    for (node_id, heading, line, body) in rows {
        dest.execute(
            "INSERT INTO sections(node_id, heading, line, body) VALUES(?1, ?2, ?3, ?4)",
            turso::params![
                Value::Text(node_id),
                Value::Text(heading),
                Value::Integer(i64::from(line)),
                Value::Text(body)
            ],
        )?;
    }
    Ok(n)
}

fn copy_embeddings(src: &Index, dest: &Index) -> Result<usize> {
    let rows = src.query_rows(
        "SELECT node_id, section_line, content_hash, vector
         FROM embeddings ORDER BY node_id, section_line",
        (),
        |r| {
            Ok((
                col_text(r, 0)?,
                col_u32(r, 1)?,
                col_blob(r, 2)?,
                col_blob(r, 3)?,
            ))
        },
    )?;
    let n = rows.len();
    for (node_id, section_line, content_hash, vector) in rows {
        dest.execute(
            "INSERT INTO embeddings(node_id, section_line, content_hash, vector)
             VALUES(?1, ?2, ?3, ?4)",
            turso::params![
                Value::Text(node_id),
                Value::Integer(i64::from(section_line)),
                Value::Blob(content_hash),
                Value::Blob(vector)
            ],
        )?;
    }
    Ok(n)
}

/// Carry provenance meta across, rewrite `deps_snapshot` to drop machine paths, and stamp
/// the packing vaire's version.
fn copy_meta(src: &Index, dest: &Index, packed_by: &str) -> Result<()> {
    for key in [
        "last_indexed_commit",
        "index_source",
        "package_name",
        "embed_provider",
    ] {
        if let Some(value) = src.meta(key)? {
            dest.set_meta(key, &value)?;
        }
    }
    if let Some(snapshot) = src.meta("deps_snapshot")? {
        // Typed on purpose: a snapshot entry missing its identity must fail here, at
        // pack time, rather than ship as a well-formed-but-null dependency record.
        #[derive(serde::Deserialize, serde::Serialize)]
        struct SnapshotDep {
            name: String,
            version: String,
            /// `null` for transitive members — their constraints live in their owners.
            constraint: Option<String>,
            /// The machine-local root: read so its presence is tolerated, never written
            /// — an artifact records choices, not locations.
            #[serde(default, skip_serializing)]
            #[allow(dead_code)]
            root: Option<String>,
        }
        let entries: Vec<SnapshotDep> = serde_json::from_str(&snapshot)
            .map_err(|e| VaireError::IndexCorrupt(format!("deps_snapshot meta: {e}")))?;
        dest.set_meta(
            "deps_snapshot",
            &serde_json::to_string(&entries).expect("json array"),
        )?;
    }
    dest.set_meta("packed_by", packed_by)?;
    Ok(())
}
