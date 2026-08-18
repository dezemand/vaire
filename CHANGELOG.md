# Changelog

The format loosely follows [Keep a Changelog](https://keepachangelog.com); this project
uses [Semantic Versioning](https://semver.org).

## [0.3.0] — 2026-08-18

**0.3 is the distribution release.** A package can now be released, published, and pulled
by somebody who has never seen your checkout — with a static file host as a full citizen of
the protocol. See [`spec/registry.md`](spec/registry.md) for the whole layer.

### Added
- **`vaire release`** (cli.md §4.7) — cut a release in one command: classify, compute the
  version, write the manifest and a release record, commit, tag. **The version is computed,
  not typed.** The classifier diffs the last release's entity index — rebuilt from its tag,
  so no old artifact is retained — against the tree: addresses added → MINOR, content
  changed → PATCH, addresses removed or `superseded_by:` appeared → MAJOR. "Content" is what
  a reader notices; a moved file or a touched `updated:` is never a release.

  MAJOR is never automatic: it exits `7` (its own code, so a pipeline reports *pending a
  maintainer* rather than *broken*) and requires `--major` with `--notes <file>`. A PATCH
  touching an entity with ten or more inbound references asks before proceeding; `--yes`
  and the absence of a terminal both skip the question. Gated on a clean tree, the mainline
  (`--allow-branch` escapes), and `vaire check` free of violations. It never runs
  `git push`.
- **Release records** — the changelog, written as corpus. Each release writes
  `releases/<version>.md`: an entity carrying the date, the bump, and **edges** to what was
  added, changed and retired. So "which releases touched this entity?" is `vaire backlinks
  <id> --type release`, and a consumer's adopted-changes digest is an intersection rather
  than a diff. Removed addresses are recorded as text — a deleted entity has no address left
  to point at. `release_type`/`release_dir` rename them for a package whose vocabulary
  already means something by the word.
- **`vaire status` reports the pending release** — `release: would be minor — 3 new, 12
  changed` — so a release is never a surprise. Silent while the index is behind HEAD, where
  the answer would describe neither tree.
- **`vaire pack` ships in the binary** (cli.md §4.6), on by default rather than behind a
  feature: the rest of the line is built against the artifact contract. Builds
  `.vaire/dist/<name>-<version>.tgz` from the committed tree — manifest, the files the globs
  select, **every file those reference** (transitively, so nothing unreferenced ships), and a
  freshly exported index. Reproducible: sorted entries, commit-pinned timestamps, zeroed
  ownership, untimestamped gzip. The feature name survives so a dependent that only reads a
  corpus can compile the artifact layer out.
- **The catalog** (cli.md §4.8) — machine-local state recording what packages this machine
  knows and where they live, in the new **vaire home** (`~/.vaire`, `VAIRE_HOME` overrides).
  `vaire catalog list | add [path] | scan <dir> | rm <path|name> | rm --missing`.

  A row is a **sighting** — "a package declaring *N* at *V* was seen at *P*" — keyed by
  canonicalized path, so two live paths declaring one name are two observations rather than a
  write-time conflict. **Two states, no clocks**: `live` or `missing`, re-checked on read,
  never expired on a timer and never removed behind your back. **Registration is ambient** —
  `index`, `check` and `add` record the package they ran in plus every working copy in its
  closure — so ordinary use fills it and no workflow gains a ceremony step. `--no-register`
  skips, and skipping never forgets.
- **Remote registries** (cli.md §4.9–§4.11) — `vaire registry add | list | show | rm`,
  `vaire push`, `vaire yank`. The first implementation needs **no server**: a registry is a
  few JSON documents and some tarballs under one base URL, so a bucket, a web root, or a
  plain directory over `file://` are all first-class. `vaire registry add lab ./registry` is
  the entire setup.

  The two guarantees a registry must make are **properties of a write, not policy checks**: a
  create-only `PUT` makes a published `(name, version)` immutable because storage itself
  refuses the second one, and a compare-and-swap on the index document turns two simultaneous
  publishers into a retry instead of a lost release. Anything beyond those is a declared
  capability, so a static host says `search: none` and the client degrades rather than fails.

  **`push` publishes tags, not the working tree.** It rebuilds each artifact from that tag's
  own tree, so `.vaire/dist/` is a cache and a container that cloned thirty seconds ago can
  publish; because `pack` is deterministic, the checksum a lockfile pins belongs to the
  release rather than to whoever uploaded it. **`yank` is an index edit** — the artifact never
  moves, so anything already pinned to it keeps resolving; only a *new* resolution changes.
  Access (`--access open | restricted | unlisted`) is per (package, registry) and, on a static
  host, **advisory** — it says so at the moment you set one.
- **The store, and `vaire pull`** (cli.md §4.12) — a pulled release is verified, unpacked,
  re-indexed *here*, and sealed read-only under `~/.vaire/store/`, where it becomes a package
  directory like any other.

  **The shipped index is a claim, never truth**: materialization throws it away and rebuilds
  from the shipped Markdown, because adopting it would make your answers depend on a
  stranger's build. Provenance is carried — which commit these files are — since that is a
  fact about the release rather than a claim about the graph. **Nothing is fetched silently**:
  resolution reports the `vaire pull` that would satisfy a missing dependency and stops.
  Resolution order is explicit link → run root → catalog → store, so a working copy outranks a
  pulled release of the same name. Retention is one slot per major line.
- **`knowledge.lock`, `pull --locked`, and `--frozen`** (cli.md §4.13) — reproducibility. The
  lockfile is written by `pull` and by indexing, never by hand, and records the whole closure.
  **Only one kind of entry carries a checksum**: a store entry can be fetched again anywhere;
  a working copy records its version and nothing else, because a checkout has no artifact to
  checksum and can change between two runs.

  `pull --locked` verifies against the digest the lockfile **recorded**, not the one the
  registry currently publishes — the only way to notice a registry serving different bytes
  under a version it already published. `--frozen` answers only from the store and refuses a
  working copy, naming the `vaire pull` that would fix it; it never consults the catalog,
  which is the index of working copies and precisely what the mode refuses. The expected
  posture for agents and CI.
- **`vaire pin` / `vaire unpin`, and `vaire clean`** (cli.md §4.14–§4.15). A pin holds one
  exact version: resolution takes it over anything newer, retention keeps it, and `clean`
  roots it. It lives in `knowledge.lock`, so it travels with the package.

  **A pin selects a release, not a world** — an explicit link and a working copy still outrank
  the store, because a committed pin that displaced checkouts would reach every colleague's
  setup. **A refresh never moves a pin**, and a pull that fetched something newer says the pin
  is why nothing changed. **You can only pin what you have**, since the entry carries the
  artifact's digest.

  `clean` keeps what a registered workspace's lockfile names, what any workspace pins, and
  what was pulled by name from outside a package — that last one because a reader with no
  package of their own is recorded by no lockfile anywhere. `vaire clean <name>` says you are
  done with one. Everything removed is still published.
- **The adopted-changes digest.** A pull that replaces a version reports what changed *that
  this package cites*, rather than the publisher's changelog:

  ```text
  acme-glossary  1.1.0      from 'lab'
      replaced 1.0.0
      1 of the 4 entities touched by 1.0.0→1.1.0 cited here:
        changed  term:torque-vectoring
  ```

  Both halves were already in the graph, so this is their intersection — short by
  construction and specific to you. Nothing about it can fail a pull.
- **Reading without a package to stand in** (cli.md §6.8). Run a read command where there is
  no `knowledge.toml` above you and the scope becomes the **catalog** — every live package
  this machine knows:

  ```bash
  vaire catalog scan ~/Documents/Knowledge
  vaire mcp                                 # every package, served to an agent
  ```

  `vaire mcp` outside a package is the point: an agent gets pointed at the machine rather than
  at one checkout, with no manifest and no install. From inside a package, `search --all` /
  `suggest --all` reach past the closure the same way.

  **Author mode never reaches the catalog.** A declared-but-unlinked dependency, or a
  reference to an undeclared package, fails exactly as before even when the catalog could
  answer — a manifest that silently resolved from ambient machine state would stop meaning
  anything to the next person who clones it. Reading is rescued only where there is no
  manifest to betray. A bare id is refused rather than reported missing, since `type:id` means
  "in this package" and there is no this package.
- **Diagram references become graph edges** (design.md §6, cli.md §3.2, issue #23). A link
  target inside a diagram source is a reference like any other, with `ref_type: diagram`.
  Vairë does not parse PlantUML, Mermaid or draw.io: the target carries a `vaire/` marker, so
  one scan works identically across every diagram language that can hold a link.

  Both shapes are covered — a fenced ```` ```plantuml ````/```` ```mermaid ```` block in a
  node's own file, and an external `.puml`/`.mmd`/`.drawio` **that the node's prose points
  at**, which is what makes a diagram belong to a node. A diagram edge reports the line
  *inside the diagram source*, and `check`'s drift rule ignores it: a diagram edge is not
  fixable the way a prose one is.
- **`vaire release --summary <file>`** (cli.md §4.7) — narration in the release record,
  written by somebody other than the classifier. **Vairë does not call a model, and gains no
  way to**: the seam is a file, so the producer is somebody else's business and a package
  stays releasable by a maintainer who has no model at all.

  What makes outside prose safe to admit is that the record's claims about itself stay
  computed. A summary may retitle the record and extend aliases, but naming `id`, `type`,
  `date`, `bump`, `added`, `changed` or `retired` is refused *by name*. `check` runs again
  with the record on disk: any violation is the summary's doing, and the record is rolled back
  to a byte-identical tree. `generated_summary: true` marks the result.
- **`vaire release --push`** stops being reserved: it runs the upload once the tag exists.
- **`repository` manifest field** (manifest.md §3) — where the package is authored, for the
  registry's pull-to-read vs clone-to-author choice.

### Changed
- **Dependency resolution goes through the catalog, and `^MAJOR` now *selects***
  (cli.md §6.6). A declared dependency with no link is satisfied by asking the catalog for a
  package declaring that name **in the constrained major line**, where v0.2.0 walked a
  configured root and matched on the name alone. Two clones at 1.4 and 2.0 therefore stop
  being an ambiguity. Versions compare as parsed triples, so `1.10.0` is above `1.9.0`.

  **The catalog is an index, never truth**: a candidate's manifest is re-read before anything
  is linked, and what it says is written back — a version bumped by a release is adopted, a
  renamed package's row follows it, a vanished path is marked missing. Nothing needs a rescan
  to heal.

  **What survives the constraint is refused, not tiebroken** — two satisfying paths are a fork
  beside its original, and a fork routinely outruns what it forked from. Both are named. An
  explicitly `catalog add`-ed path outranks one something noticed in passing. **Constraints
  across a closure are intersected**, since one directory is linked per name; disjoint majors
  are reported naming both declarers, and any link the pass made for that name is withdrawn.
- **`vaire release --dry-run` reports the notes a MAJOR owes rather than refusing over them**
  (`notes_required` in JSON). A dry run writes nothing, so this is a faithful prediction
  rather than a loosened gate — and obtaining the plan in order to *write* those notes was
  previously impossible. The real run still refuses.
- **The storage facade moved to `vaire::db`**, shared by the package index and the catalog, so
  there is one async→sync bridge rather than two copies of its traps.

### Removed
- **`local-packages` is retired** — the `vaire configure local-packages` command, the setting,
  and the discovery walk on the resolution path (cli.md §6.7). The walk survives only inside
  `vaire catalog scan`, demoted from resolution machinery to an import tool. The first
  maintain command after upgrading **migrates itself**, and says so; dependencies keep
  resolving across the upgrade. The walk conflated "on my disk somewhere" with "I author
  this"; the catalog does not.

### Fixed
- **TLS trusts what the operating system trusts.** HTTPS previously validated against a
  bundled root snapshot (`webpki-roots` 0.26, a line frozen upstream), which no longer
  contains the root GitHub's newer certificate chains use — so `vaire upgrade` could not
  download release assets and a registry on `raw.githubusercontent.com` could not be
  pulled, while `api.github.com` (an older root) still worked. All HTTP now validates
  against the platform trust store instead, which also follows corporate-proxy roots the
  OS has been told to trust. Note for 0.2.1 users: the old binary carries the frozen
  snapshot, so `vaire upgrade` cannot fetch this release — reinstall via the install
  script once.
- **Cross-process safety for shared state, measured rather than assumed.** The catalog design
  assumed short WAL transactions made concurrent invocations safe. They do not: **the storage
  engine takes an exclusive lock when a database is opened**, so a second process cannot open
  it at all, reads included. A multi-process test proved it before anything relied on it, and
  caught a bug it would otherwise have shipped — an `open` that treated every connect failure
  as corruption would have *deleted the catalog* whenever another process held it.

  That lock is now the cross-process mutex: contention is retried with bounded backoff,
  connections are short-lived and lazily opened. Eight processes making 400 concurrent writes
  land every one. The cost is recorded rather than hidden — catalog access is serialized
  machine-wide, so nothing may hold a handle resident.
- **The catalog is never destroyed on a guess.** Recreating it stopped being free once
  `vaire clean` began reading its roots from those rows: most are observations a rescan
  reproduces, but a pin and a pulled-by-name record are not, and losing them means the next
  sweep deletes the releases they were holding. So **unreadable must now be proven, not
  inferred** — a failure to connect only says the engine did not get a database, so the file
  is re-opened directly and only one this process can itself read and write is treated as
  garbage. A catalog that is merely unreachable (permissions, a half-mounted home) is an
  error, and an error deletes nothing. A catalog from a **newer vaire is refused rather than
  rebuilt**, which is the lockfile's rule applied to the same problem: declining to *read* a
  format you do not know is only coherent if you also decline to *overwrite* it. What does
  get displaced is kept beside the catalog as `catalog.db.unreadable`, so a wrong guess costs
  a file to look at rather than the record of what this machine holds — each displacement
  taking its own generation, since a fixed name would let a second corruption drop the newly
  rebuilt catalog on top of the one still holding the pins somebody wanted back. A file that
  cannot be moved aside is left alone rather than removed: downgrading that to a deletion
  would defeat the point in exactly the case that most warrants care.
- **A first release records what it publishes.** It has no baseline to diff, which is not the
  same as having nothing to say: everything the corpus holds is what that release published,
  and it is now recorded as `added`. The empty version was not neutral — a record's edges are
  how "which release published this?" is answered, and no later release re-adds an entity that
  was already there, so every founding entity was left permanently unattributed in the one
  release where they are all of them.

  Its consequence is fixed with it: a release record may cite an entity a later release
  removed. Such an edge is a statement about the past, not a broken reference — it cannot be
  corrected (records are immutable) and it had no author to have mistyped it (the classifier
  wrote it), so treating it as dangling would fail `check` forever and, since `release` gates
  on `check`, strand the package permanently. The exemption covers the classifier's own
  entries and nothing else: an address a `--summary` *invented* still dangles and still
  refuses the release.
- **An artifact no longer records who built it.** The packed index stamped the packing
  vaire's own version, so the same tag packed to different bytes after an upgrade — and since
  that digest is what a lockfile pins and what `push` re-derives from a tag, a re-push then
  reported the release as somebody else's, and `pull --locked` raised its
  registry-tampering alarm on a false positive. What ships instead is `artifact_format`, the
  artifact layout's own version, which changes only when the layout deliberately does.
- **A publish interrupted before its index write can be finished.** Only the first of the
  three writes is atomic, so the window between them is real, and refusing what it leaves
  behind made that state *permanent*: the create-only `PUT` means the identity is immutably
  taken, so no later push could ever claim it. A push that finds its own bytes already placed
  and unrecorded now completes the job rather than reporting a dead end. Decided by the bytes,
  never by the gap — a version occupied by somebody else's artifact is still refused.
- **Package names in a registry's enumeration document are validated** before they become
  path segments (§10.2). The document is written by whoever publishes to the registry, so a
  name in it is input like any other; every other entry point already checked, and this one
  did not.
- **`vaire catalog add` refuses a release in the store, and `scan` walks past one** (§4.3).
  A store entry is a package directory, so nothing about the path refuses it — which is
  precisely why the command has to. A sighting claims "observed at a path, and may have
  changed since"; a sealed release is the opposite claim, and recording one would make a
  single directory arrive under two identities, the second outranking the store as a working
  copy somebody edits.
- **A diagram marker that does not parse is reported** (design.md §6, new
  `malformed_diagram_ref` warning). It had nowhere to surface: not an edge, since there is no
  address to point at, and not a loose end, since `[[?type: descriptor]]` needs a space and a
  diagram is deliberately not where an open question is recorded. So a typo in a diagram
  simply evaporated, against the promise made in as many words. Index schema v7.

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
