---
id: index-engine
type: component
name: Index engine
aliases: [Turso index]
---
# Index engine

The storage layer underneath every read command: an edges table
(`from_id, to_id, ref_type, source_file, line`), a `sections` table carrying a native
full-text index over prose, and a native vector column holding per-section embeddings — all
in one [[decision:turso-engine|Turso]] database at `.vaire/index.db`. A single
`schema_version` row stamps the whole file, so any future build of `vaire` can tell how to
migrate what it finds without guessing at the shape underneath.

Building or rebuilding it is [[cli:vaire/command:index]]'s job exclusively; every other
command only ever opens it read-only or via `--working-tree` for the edit-validate loop. A
schema mismatch is never silently patched — the next plain build always rebuilds from
scratch, which is consistent with treating the whole thing as a
[[concept:derived-index|derived, disposable]] cache rather than state to migrate carefully.
