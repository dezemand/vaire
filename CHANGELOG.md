# Changelog

The format loosely follows [Keep a Changelog](https://keepachangelog.com); this project
uses [Semantic Versioning](https://semver.org).

## [Unreleased]

### Added
- **`vaire pack`** (cli.md §4.6) — **behind the non-default `pack` feature**, so it
  is not compiled into the released binary: the artifact format is still settling and
  the registry that consumes it does not exist yet. Build with `--features pack` to
  work on it. Build the package's distributable artifact
  (`.vaire/dist/<name>-<version>.tgz`) from the committed tree: the manifest, the
  corpus files the include/exclude globs select, **every file those reference** by
  relative link or image (inline and reference-style, transitively through referenced
  Markdown — no reserved directory, and an unreferenced file never ships), and a
  freshly exported `.vaire/index.db`. Gated on `vaire check`. A link target missing at
  HEAD fails the pack; targets the author chose to keep out — exclude-glob-vetoed or
  gitignored — warn, as does a dirty working tree. Reproducible: sorted entries,
  commit-pinned timestamps, zeroed ownership, untimestamped gzip, pinned compression
  backend — and the exported index ships no `embed_cache`, no machine paths in
  `deps_snapshot`, and no FTS structure (recreated at materialization).
  `--no-embeddings` strips section vectors.
- **`repository` manifest field** (manifest.md §3) — where the package is authored, for
  the registry's pull-to-read vs clone-to-author choice (registry design v0.2).

## [0.2.1] — 2026-08-03

### Added
- **Cargo features** — the crate's parsing core (`model`, `corpus`, `config`, `error`)
  can now be used without the index layer. `index` is a **default** feature covering
  the Turso-backed index, search, embeddings, git provenance, the CLI binary and the
  MCP server, so the binary and every existing dependent are unaffected. With
  `default-features = false` a library consumer gets the reference grammar,
  frontmatter parsing, the manifest and discovery globs while dropping turso, tokio,
  clap, rustls and ~350 other crates — 67 crates in the tree instead of 420. Added for
  `vaire-renderer`, which needs the corpus semantics and nothing else; the point is
  that a consumer reuses this grammar rather than reimplementing it.

### Changed
- `vaire pack` and its artifact-index writer are now behind a non-default `pack`
  feature, so they are compiled out of the released binary. `pack` landed after
  `v0.2.0` was tagged and has never shipped, so nothing is removed from a released
  surface.

## [0.2.0] — 2026-07-23

The knowledge-package release: corpora become **packages** that reference each other on
the local filesystem — `@acme-core/team:platform` resolves, searches, and lints across
package boundaries, with no registry and no network.

### Added
- **`knowledge.toml`** — the committed package manifest (name, semver version, `types`
  vocabulary, include/exclude, `[dependencies]` with `^MAJOR` constraints) replaces
  `.vaire/config.toml` and is the corpus discovery marker; `vaire init` migrates legacy
  configs.
- **`vaire configure`** + a global user config: embedding settings move out of the
  package manifest (a package must not dictate how a consumer indexes it); secrets in a
  `600` `credentials.toml`, env vars winning.
- **Strict reference grammar** — a reference is identifiable by *shape alone*
  (identification), with the `types` vocabulary consulted afterwards (classification).
  Kills the `url:` false-positive class; new `unreferenceable_id` warning for declared
  ids nothing can address.
- **Cross-package references** — `@<package>/type:id`, always explicitly qualified;
  recorded in the index (`nodes.package`, `edges.to_package`) and resolved through the
  *referencing* package's own `[dependencies]`, including `superseded_by` tombstones that
  hop packages.
- **Local packages** — `vaire configure local-packages <dir>` records where your packages
  live on this machine, and declared dependencies are then satisfied from there
  automatically: a fresh clone is `vaire index`, with no per-checkout wiring step.
  Packages are matched by the name their manifest **declares**, at any depth, so a
  knowledge base nested inside a bigger repo is found like any other; an ambiguous name is
  reported rather than guessed between, an explicit link always wins, and a broken link
  heals. Where a package lives stays a machine setting — the committed manifest never
  carries a path.
- **Linked packages** — a dependency lives at `.vaire/packages/<name>` (npm-style
  symlink, per-checkout, never committed); `vaire add <pkg>[@^N] --link <path>` declares
  and wires it explicitly. The index stays **federated**: every package keeps its own
  `.vaire/index.db`, so ids never collide and a dependency's embeddings are computed once
  for all consumers. `vaire index` refreshes the linked closure (`--no-deps` skips).
- **All read commands cross packages** — `resolve`, `render`, `backlinks`, `refs`,
  `search`, `suggest` (dependency hits arrive pre-qualified), `unresolved
  --all-packages`; cross-package results carry a `package` field, and unavailable
  dependencies are surfaced under `skipped`, never silently dropped.
- **`vaire deps`** — the resolved local dependency tree (live link inspection, no index
  needed); also an MCP tool.
- **`vaire upgrade`** — self-update from GitHub releases, following the installer
  scripts' contract (latest release → platform asset → atomic binary swap). Semver
  gated: installs only when the release is higher than the running build, never
  downgrades unless a version is pinned explicitly; refuses in package-manager-owned
  locations and names that manager's own upgrade command instead. The installer
  scripts got the same semantics (bare versions with `v` accepted, already-up-to-date
  no-op).
- **Resolution lints in `vaire check`** — dangling cross-package references,
  `undeclared_import`, `missing_dependency` (errors); `unused_dependency` and
  `dependency_version_mismatch` (warnings; enforcement is future work). Check runs the
  dependency ensure pass first, so it works from a cold clone.
- **`vaire status`** — per-dependency rows: index freshness, commit lag, embedding
  provider (with an explicit note when vector search silently skips a mismatched dep).
- `examples/` — both runnable examples in one place: `corpus/` (the single-package model,
  moved from the top-level `example/`) and `workspace/` (a three-package workspace
  exercising all of the above). Each is built by a release-gate test, so a shipped example
  cannot rot silently.

### Changed
- **Index engine: `rusqlite`/FTS5 → Turso Database** (the Rust rewrite of SQLite), as a
  local embedded file engine: native FTS (per-field BM25 weights) and native exact
  vector search replace the FTS5 virtual table and the hand-rolled cosine. Schema v3;
  existing indexes rebuild automatically.
- Reference targets are strictly validated (`[a-z][a-z0-9-]*` types,
  `[a-z0-9][a-z0-9-]*` ids); values that fail the grammar are plain scalars, never
  guessed at.
- The index records which embedder produced its vectors (`embed_provider`); a provider
  switch forces a clean re-embed of dependency indexes instead of mixing vector spaces.

## [0.1.0] — 2026-06-16

Initial release.

- **CLI:** `init`, `index` (`--full` / `--working-tree` / `--re-embed`), `check`
  (`--strict`), `status`, `resolve`, `render`, `backlinks`, `refs`, `search`, `suggest`,
  `unresolved`, and `mcp`.
- **Model:** typed `type:id` nodes (frontmatter `id:` + `type:`), frontmatter edge lists and
  inline `[[wikilinks]]`, optional `name:` (falls back to `# H1` then filename), unresolved
  `[[?type: descriptor]]` loose ends, `superseded_by` redirects, and opt-in scoped IDs
  (`<container-id>/type:local`).
- **Index:** derived SQLite (edges, FTS5, per-section embeddings, content-hash cache),
  commit-bound with a working-tree mode; integrity checks (duplicate IDs, dangling refs,
  orphans, drift, frontmatter-`[[ ]]`, unknown types).
- **Embeddings:** pluggable — built-in local, shell `command`, and OpenAI (with `.vaire/.env`
  secrets).
- **MCP:** STDIO server exposing the read commands as tools, one-to-one with the CLI.
- **Skills:** `vaire-files`, `vaire-query-cli`, `vaire-query-mcp` under `skills/`.
