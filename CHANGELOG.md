# Changelog

The format loosely follows [Keep a Changelog](https://keepachangelog.com); this project
uses [Semantic Versioning](https://semver.org).

## [Unreleased]

### Changed
- **Dependency resolution goes through the catalog, and the `^MAJOR` constraint now
  *selects*** (cli.md §6.6). A declared dependency with no `.vaire/packages/<name>` entry
  is satisfied by asking the catalog for a package declaring that name **in the
  constrained major line** — where v0.2.0 walked a configured root and matched on the name
  alone. Two clones of one package at 1.4 and 2.0 therefore stop being an ambiguity: a
  consumer declaring `^1` has already said which it wants, and the `^2` consumer beside it
  gets the other. Versions compare as parsed triples (`1.10.0` above `1.9.0`, and `^0` is
  a major line like any other) — the string-prefix comparisons in `deps` and `check` are
  gone with it.

  **The catalog is an index, never truth**, so a candidate's `knowledge.toml` is re-read
  before anything is linked and what it says is written back: a version bumped by a release
  is adopted on the spot, a renamed package stops matching its old name (its row follows it
  rather than being deleted), and a path that no longer answers is marked `missing`.
  Nothing needs a rescan to heal.

  **What survives the constraint is refused, not tiebroken.** Two live paths that both
  satisfy are a fork beside its original, or two worktrees — and a fork routinely outruns
  what it forked from, so picking the higher version would be a guess dressed as
  arithmetic. Both paths are named. One tier applies first: a path explicitly
  `vaire catalog add`-ed outranks one a scan or a passing command noticed, which settles an
  ambiguity without editing any consumer's links.

  **Constraints across a closure are intersected.** One directory is linked per name, so
  when several members constrain one dependency they must agree on the major. Disjoint
  majors are reported naming both declarers — and any link the pass had already made for
  that name is withdrawn, since leaving an answer wired up that one declarer cannot use
  would be worse than the version-blind lint this replaces. Conflicts are judged after the
  links settle, over the whole closure: judged mid-walk, one would be invisible whenever
  the first constraint seen happened to resolve, making the outcome depend on link order.
  An explicit `--link` is the escape hatch and is never withdrawn.

  Everything downstream is untouched: `.vaire/packages/<name>` still points at a directory,
  the resolver and every read command neither know nor care who put it there, and reads
  still never materialize a link (nor take the catalog's lock).

### Removed
- **`local-packages` is retired** — the `vaire configure local-packages` command, the
  setting, and the discovery walk on the resolution path (cli.md §6.7). The walk survives
  only inside `vaire catalog scan`, demoted from resolution machinery to an import tool.
  The first maintain command after upgrading **migrates itself**: whatever the old root
  held is imported into the catalog once, the `[packages] local` key is dropped, and the
  run says so. Dependencies keep resolving across the upgrade, and nothing walks anything
  again. The walk conflated "on my disk somewhere" with "I author this"; ambient
  registration and the catalog do not.

### Added
- **The catalog** (cli.md §4.8) — machine-local state recording what packages this machine
  knows and where they live, in the new **vaire home** (`~/.vaire/catalog.db`,
  `VAIRE_HOME` overrides). `vaire catalog list | add [path] | scan <dir> | rm <path|name>
  | rm --missing`. Package-level metadata only: every per-package index still lives beside
  its package, and nothing here holds entity content.

  A row is a **sighting** — "a package declaring *N* at *V* was seen at *P*" — keyed by
  canonicalized path, with the name as an attribute, so two live paths declaring one name
  are two observations rather than a conflict to resolve at write time. Rows are
  observations throughout: a corrupt catalog is recreated rather than repaired, because
  losing it costs a rescan and nothing else.

  **Two states, no clocks**: `live` or `missing`. Nothing expires on a timer and nothing is
  removed behind your back; `list` re-checks each path as it goes, so a vanished checkout
  shows `missing` and a returning one flips back to `live`. Sweeping is explicit
  (`rm --missing`).

  **Registration is ambient** — `index`, `check`, and `add` record the package they ran in
  plus every working copy in its dependency closure, so ordinary use fills the catalog and
  no workflow gains a ceremony step. `--no-register` skips on all three; skipping never
  forgets, and an ambient touch never demotes a hand-registered row. A catalog that cannot
  be written warns and is otherwise ignored.

  Dependency resolution consults it (see *Changed*), and it is the enumerable scope the
  rootless reader will fan out over.
- **`vaire catalog scan <dir>`** — bulk import, and the one place the old discovery walk
  now lives (same depth and skip rules), demoted from resolution machinery to a one-shot
  import tool.
- **`vaire release`** (cli.md §4.7) — cut a release in one command: classify what changed
  since the last one, compute the version, write the manifest and a release record,
  commit, tag. **The version is computed, not typed.** The classifier diffs the entity
  index of the last release — rebuilt from that release's tag, so no old artifact needs
  retaining — against the current tree: addresses added → MINOR, content changed →
  PATCH, addresses removed or a `superseded_by:` appeared → MAJOR. "Content" is what a
  reader notices (sections, edges, aliases); a moved file or a touched `updated:` is
  bookkeeping and never a release. A rename needs no special case — an address *is* the
  identity, so it presents as a removal plus an addition. A package with no prior tag
  publishes the version its manifest already declares.

  **MAJOR is never automatic**: it exits `7` — its own code, so an automated pipeline
  reports *pending a maintainer* rather than *broken* — and requires `--major` together
  with `--notes <file>`, the invalidated assumptions dependents read to decide about
  re-confirmation. `--major` also escalates a small edit that reverses a truth, because
  the maintainer owns meaning while the tool owns structure.

  **Backlink weighting** is the one advisory: a PATCH touching an entity with ten or more
  inbound references reports it (`department:platform has 14 inbound references — patch,
  really?`) and asks before proceeding, because structure is only a proxy for meaning.
  `--yes` skips the question (the CI posture) and so does the absence of a terminal — a
  prompt that blocked an automated release would be a bug — while the advisory still
  rides along in the output.

  Gated on a clean tree, HEAD on the mainline (`--allow-branch` escapes), the package
  being its own Git repository, and `vaire check` free of violations. Nothing to release
  is a clean no-op, exit `0`. It never runs `git push` and never uploads: git transport
  stays the maintainer's, and publishing is `vaire push` (not yet implemented), so a
  flaky upload re-runs an upload rather than a ritual and CI can publish a tag it did not
  cut. `--push`/`--onto` are reserved grammar, rejected as not yet implemented.
- **Release records** — the changelog, written as corpus. Each release writes
  `releases/<version>.md`: an entity carrying the date, the bump, and **edges** to what
  was added, changed, and retired, so "which releases touched this entity?" is
  `vaire backlinks <id> --type release` and a consumer's adopted-changes digest becomes
  an intersection rather than a diff. Removed addresses are recorded as text — a deleted
  entity has no address left to point at. The classifier **excludes** release records
  from its own diff, or no release after the first could ever be a PATCH. New manifest
  keys `release_type`/`release_dir` (defaults `release`/`releases`) rename them for a
  package whose own vocabulary already means something by the word.
- **`vaire status` reports the pending release** — `release: would be minor — 3 new, 12
  changed` — so a release is never a surprise. Best-effort and cheap: the common "nothing
  new since the last release" case is a commit count, and it stays silent while the index
  is behind HEAD, where the answer would describe neither tree.
- **`vaire pack` ships in the binary** (cli.md §4.6). It landed in 0.2.1 behind a
  non-default feature, because the artifact format was still settling and nothing
  consumed it. Both reasons are spent: the rest of the registry line is built against
  this contract — `release` tags what `pack` builds, and publishing re-packs from a tag
  — so it is on by default and compiled into every released binary. The feature name
  survives, so a dependent that only reads a corpus can still compile the artifact layer
  out. Build the package's distributable artifact
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

### Changed
- **The Turso facade moved to `vaire::db`**, shared by the package index and the catalog,
  so there is one async→sync bridge rather than two copies of its traps. `Index` keeps its
  public surface.

### Fixed
- **Cross-process safety for shared state, measured rather than assumed.** The catalog
  design assumed short WAL transactions made concurrent CLI invocations safe. They do not:
  **Turso takes an exclusive lock when a database is opened**, so a second process cannot
  open it at all — reads included. A multi-process test (`tests/catalog_concurrency.rs`)
  proved it before anything relied on it, and caught a bug it would otherwise have shipped
  — an `open` that treated every connect failure as corruption would have *deleted the
  catalog* whenever another process held it. Turso stays: that lock is the cross-process
  mutex, and an OS file lock cannot outlive its process, so contention is now retried with
  bounded backoff and connections are short-lived. Eight processes making 400 concurrent
  writes now land every one of them. The cost is recorded rather than hidden — catalog
  access is serialized machine-wide, so nothing may hold a handle resident.

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
