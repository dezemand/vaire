# Changelog

The format loosely follows [Keep a Changelog](https://keepachangelog.com); this project
uses [Semantic Versioning](https://semver.org).

## [Unreleased]

### Added
- **Reading without a package to stand in** (cli.md §6.8). Every read command has assumed
  an author: scope is the package you are in, plus what its manifest declares. That serves
  the person writing a package and offers nothing to the larger audience who authors
  nothing and just wants to ask questions across everything they have. Run a read command
  where there is no `knowledge.toml` above you and the scope becomes the **catalog** —
  every live package this machine knows:

  ```bash
  cd ~
  vaire search "incident review"            # every catalogued package
  vaire resolve @acme-core/team:platform    # by name, declared by nobody
  vaire mcp                                 # the same scope, served to an agent
  ```

  `vaire mcp` outside a package is the point of the whole thing: an agent gets pointed at
  the machine rather than at one checkout, with no manifest and no install. From *inside* a
  package, `search --all` / `suggest --all` reach past the closure the same way — useful
  when the answer lives somewhere you never declared a dependency on.

  Everything is package-qualified, because with no package you are standing in, nothing is
  local. A **bare id is refused rather than reported missing**: `type:id` means "in this
  package", and there is no this package, so the error says that and shows the qualified
  form instead of implying the node does not exist.

  **A package's own references still mean what its author meant.** Following a reference
  out of a catalogued package uses *that* package's `[dependencies]` and its own links
  first; the catalog is consulted only where an ordinary session would have run out of
  places to look. And the fallback runs one way only: **author mode never reaches the
  catalog.** A declared-but-unlinked dependency, or a reference to an undeclared package,
  fails exactly as before even when the catalog could answer — a manifest that silently
  resolved from ambient machine state would stop meaning anything to the next person who
  clones it, and `vaire check` would be a different question on every machine. Reading is
  rescued only where there is no manifest to betray.

  A catalogued package whose index cannot be read is skipped and named, never fatal.
  Maintain commands are untouched: without a package they still report *no corpus found*,
  because there is nothing there for them to maintain.

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
- **The store, and `vaire pull`** (cli.md §4.12, registry.v2.md §5–§6) — the consuming half
  of the registry line. A pulled release is verified, unpacked, re-indexed *here*, and
  sealed read-only at `~/.vaire/store/<name>/<version>/`, where it becomes a package
  directory like any other: the resolver links to one exactly as it links to a checkout, and
  nothing above resolution knows the difference.

  **The shipped index is a claim, never truth.** An artifact carries a prebuilt index and
  materialization throws it away, rebuilding from the shipped Markdown. Adopting it would
  make every consumer's answers depend on a stranger's build, and a corpus whose index
  disagrees with its own text would have no way to be caught. Provenance *is* carried —
  which commit these files are — because that is a fact about the release rather than a
  claim about the graph.

  **Nothing is fetched silently.** Resolution reports the `vaire pull` that would satisfy a
  missing dependency and then stops; acquiring a package stays a decision somebody makes.
  Resolution order is explicit link → run root → catalog → store, so a working copy outranks
  a pulled release of the same name: a checkout is what you are authoring, and answering
  from a published copy of it would quietly answer against yesterday.

  Unpacking is the one place this tool treats input as hostile — entries that are absolute,
  climb out with `..`, or are links of any kind are refused, only `index.db` may appear under
  `.vaire/`, and an artifact whose manifest declares a different name than it was served
  under is refused outright. Retention is one slot per major line: pulling 1.4.2 removes
  1.4.1 and says so, which is safe because within-major substitutability is the protocol's
  own promise and free because the registry keeps every version forever.
- **Remote registries** (cli.md §4.9–§4.11, registry.v2.md §8–§9) — `vaire registry add |
  list | show | rm`, `vaire push`, `vaire yank`, and the `Registry` seam behind them. The
  first implementation needs **no server**: a registry is a handful of JSON documents and
  some tarballs under one base URL, so a bucket, a web root, or a plain directory over
  `file://` are all first-class. `vaire registry add lab ./registry` is the entire setup.

  **The two guarantees a registry has to make are properties of a write, not of a policy
  check.** A create-only `PUT` makes a published `(name, version)` immutable because storage
  itself refuses the second one; a compare-and-swap on the index document turns two
  simultaneous publishers into a retry instead of a lost release. Over `file://` the first is
  exact (`O_CREAT|O_EXCL`) and the second is compare-then-rename with a documented window —
  a lock file would need stale-lock detection, and that heuristic breaks locks it should not.
  Server *behaviors* beyond those are capabilities the registry declares, never assumptions,
  so a static host says `search: none` and the client degrades rather than failing.

  **`push` publishes tags, not the working tree.** It enumerates this package's release tags
  and rebuilds each artifact from that tag's own tree, which makes `.vaire/dist/` a cache and
  never a requirement — a container that cloned the repository thirty seconds ago can
  publish. That works because `pack` is deterministic: the artifact rebuilt from `v1.4.2` is
  byte-for-byte the one that tag produced, so the checksum a lockfile will pin belongs to the
  release rather than to whoever uploaded it. It needs no embedder and re-runs no `check`
  (the tag was already gated when it was cut), so CI publishes with no embedding
  configuration at all. Re-running it is a clean no-op, and one bad tag is reported and
  stepped over rather than stopping the rest. A version storage refuses is checked rather
  than assumed: identical bytes are idempotence, different bytes are a failure naming both
  digests, since a published version is immutable and pushing again cannot fix it.

  **`yank` is an index edit and nothing else.** The artifact never moves, so a lockfile
  pinning that version keeps resolving; what changes is only what a *new* resolution would
  choose. `--undo` exists because the reason for a yank is usually a mistake about a release
  rather than a fact about it.

  Access (`--access open | restricted | unlisted`) is per (package, registry) and sticky.
  On a static host it is **advisory** and says so at the moment it is set: restricted-listed
  is a routing workflow — its `hint` is carried verbatim into the refusal — not a wall.
- **`vaire release --push`** stops being reserved: it runs the upload once the tag exists.
  A convenience over the release/push split, not a merge of it — the tag is already cut, so
  a failed upload is one `vaire push` away.
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
  is a clean no-op, exit `0`. It never runs `git push`, and uploads only when asked: git
  transport stays the maintainer's, and publishing is `vaire push`, so a flaky upload
  re-runs an upload rather than a ritual and CI can publish a tag it did not cut. `--push`
  runs that upload once the tag is cut; `--onto` stays reserved grammar, rejected as not
  yet implemented.
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
