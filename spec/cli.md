---
title: Vairë — CLI Spec
status: draft
date: 2026-06-15
scope: The `vaire` command-line interface and its STDIO MCP surface
---

# Vairë — CLI Spec

The command surface for `vaire`, the derived index over the knowledge corpus. This spec
defines invocation, global conventions, every command, its flags, its human and JSON
output shapes, and exit codes. For the architecture behind it, see [design.md](design.md).

## 1. Model

`vaire` is a single binary with subcommands. There is exactly one implementation; the MCP
server (`vaire mcp`) re-exposes the **read** subcommands as MCP tools, so the tool surface
and the CLI cannot drift.

```
vaire [GLOBAL FLAGS] <command> [ARGS] [COMMAND FLAGS]
```

Two classes of command:

- **Read** — `resolve`, `render`, `backlinks`, `refs`, `search`, `suggest`, `unresolved`. Queries
  against the index. Available over MCP. Every read command accepts `--json`. (`render`
  is the one read that returns a file *body* rather than pointers — see §3.6.)
- **Maintain** — `init`, `index`, `check`, `status`, `configure`. Scaffold, build, validate,
  report, and set global user settings. **Not** exposed over MCP; run by humans, git hooks, or CI.

A read command run before the index exists is an error directing the user to `vaire index`
(exit `4`, see §7) — `vaire` never silently builds the index as a side effect of a query,
because indexing is bound to commit (commit-as-publish) and should be deliberate.

## 2. Global conventions

### 2.1 Repo discovery

`vaire` operates on one corpus repository. It locates the root by walking up from the
working directory to the nearest directory containing a **`knowledge.toml`** — the committed
manifest that marks a directory as a corpus (run `vaire init` to create one, §4.4). The
derived index lives at `<root>/.vaire/index.db`.

Override with `--repo <path>` or the `VAIRE_REPO` environment variable (`--repo` wins); an
explicit path that has no `knowledge.toml` is an error rather than a silent guess. If no
corpus is found and none is given, exit `4`. If a directory (or an ancestor) has a legacy
`.vaire/config.toml` but no `knowledge.toml`, the error points at `vaire init` to migrate it,
rather than reporting "no corpus".

Discovery is deliberately decoupled from Git: the corpus root need not be a Git repo root.
Whether the index is built from the committed tree or the working tree is a separate
question, decided by `vaire index` from the corpus's Git state (§4.1).

### 2.2 Global flags

| Flag | Meaning |
| --- | --- |
| `--repo <path>` | Corpus repo root. Overrides discovery and `VAIRE_REPO`. |
| `--json` | Emit JSON instead of human-readable text. Read commands only. |
| `--config <path>` | Path to the config file (default: `<root>/knowledge.toml`, see §6). |
| `--quiet` / `-q` | Suppress progress and non-essential output (errors still print). |
| `--verbose` / `-v` | Extra diagnostics on stderr. Repeatable. |
| `--no-color` | Disable ANSI color. Also honored via `NO_COLOR`. |
| `--version` / `-V` | Print version and exit. |
| `--help` / `-h` | Print help for the binary or a subcommand. |

### 2.3 Output discipline

- **stdout** carries the command's result (human text or, with `--json`, a single JSON
  value). Nothing else is written to stdout.
- **stderr** carries progress, warnings, and errors. `--quiet` silences progress and
  warnings; errors always print.
- With `--json`, stdout is **always** valid JSON — including errors, which are emitted as
  `{"error": {...}}` (§7) rather than a bare message — so a machine consumer can parse one
  shape unconditionally.
- Output is **stable and deterministic**: results are sorted by a documented key (noted
  per command) so diffs and snapshots are reproducible.

### 2.4 The returned unit: paths + IDs, not file bodies

Every read command returns **pointers** — node IDs, file paths, and (for search) section
anchors — never the prose body of a file. The index points; files hold (design.md §9).
The caller opens the file for depth. Paths are repo-root-relative POSIX paths.

## 3. Read commands

All read commands are available over MCP (§5) and accept `--json`. JSON shapes below are
the **exact** shape MCP returns.

### 3.1 `vaire resolve <id>`

Resolve a node ID to its location and frontmatter.

```
vaire resolve <id> [--json]
```

- `<id>` — a composed node ID `type:id`, e.g. `person:jane-doe`, `department:hr` — or a
  cross-package target `@pkg/type:id` (§6.5), resolved through the linked package.
- Follows `superseded_by` redirects (design.md §8) and reports the redirect in the
  result; a redirect may hop packages (the tombstone owner's dependencies apply).
- `type` is the node's `type:` field; `frontmatter.name` is the reference's default
  display text — what `[[type:id]]` renders as (design.md §6).
- Cross-package results carry a `package` field in JSON (absent for local nodes) with
  `path` staying **package-root-relative**; human output shows the clickable
  consumer-relative path (`../acme-core/knowledge/….md`) instead.
- Exit `5` if the ID is not a node; exit `4` if its package is undeclared or unavailable
  (not linked / broken link — the message names the fix).

Human:

```
person:jane-doe
  path:    knowledge/entities/people/jane-doe.md
  type:    person
  name:    Jane Doe
  aliases: Jane, J. Doe
  status:  active
```

JSON:

```json
{
  "id": "person:jane-doe",
  "type": "person",
  "path": "knowledge/entities/people/jane-doe.md",
  "frontmatter": { "name": "Jane Doe", "aliases": ["Jane", "J. Doe"], "status": "active" },
  "superseded_by": null
}
```

When the requested ID was superseded, `id` is the **target** ID, `path`/`frontmatter`
describe the target, and `superseded_by` records the chain that was followed:

```json
{ "id": "person:jane-doe", "...": "...", "requested_id": "person:j-doe-dup", "superseded_by": "person:jane-doe" }
```

### 3.2 `vaire backlinks <id>`

Nodes that reference `<id>` (inbound edges).

```
vaire backlinks <id> [--type <T>] [--limit <N>] [--json]
```

- `--type <T>` — restrict to referencing nodes of a given type (e.g. `record`).
- `--limit <N>` — cap results (default: unbounded).
- Sorted by referencing node `id` ascending (qualified ids sort by their full `@pkg/…`
  form).
- **Cross-package** (§6.5): `<id>` may be `@pkg/type:id`, and referencing nodes are
  gathered from the whole dependency closure — each member consulted through *its own*
  aliases for the target's package. Cross-package rows carry a `package` field in JSON
  (`path` stays package-root-relative); human output shows consumer-relative paths.
  Dependencies that could not be consulted are listed in `skipped`, never silently
  dropped. Inbound visibility is scoped to the closure — "you see what you depend on";
  a workspace-/registry-wide reverse query is future work (design.md §9).

JSON:

```json
{
  "id": "person:jane-doe",
  "backlinks": [
    {
      "id": "record:2026-06-10-broker-sync",
      "type": "record",
      "path": "projects/atlas/2026_q2/meeting-notes/2026-06-10-broker-sync.md",
      "ref_type": "participants",
      "line": 5
    }
  ],
  "count": 1
}
```

`ref_type` is the edge origin: a frontmatter key (`participants`, `references`, `project`)
or `inline` for a wikilink in prose. `line` is the 1-based source line.

### 3.3 `vaire refs <id>`

Nodes that `<id>` references (outbound edges).

```
vaire refs <id> [--depth <N>] [--type <T>] [--json]
```

- `--depth <N>` — traverse outbound edges N hops (default: `1`). Depth > 1 returns a
  flattened, de-duplicated node set with each node's shortest distance from `<id>`.
- `--type <T>` — restrict to referenced nodes of a given type.
- Unresolved (`[[?...]]`) references are **not** edges and never appear here; use
  `vaire unresolved`.
- Sorted by `(distance, id)`.
- **Cross-package** (§6.5): the BFS follows `@pkg/` edges through each edge's *owning*
  package (source-package keying), so traversal crosses boundaries and comes back;
  dedup is per `(package, id)`. A dangling cross-package target is dropped exactly like
  a local dangling ref (`check` surfaces them); unavailable dependencies are listed in
  `skipped`. Cross-package rows carry `package` in JSON; human paths are
  consumer-relative.

JSON:

```json
{
  "id": "record:2026-06-10-broker-sync",
  "depth": 1,
  "refs": [
    { "id": "person:jane-doe", "type": "person", "path": "knowledge/entities/people/jane-doe.md", "ref_type": "participants", "line": 5, "distance": 1 },
    { "id": "system:ingest-api", "type": "system", "path": "knowledge/entities/systems/ingest-api.md", "ref_type": "references", "line": 6, "distance": 1 }
  ],
  "count": 2
}
```

### 3.4 `vaire search <query>`

Hybrid full-text + vector search over the corpus. Returns files (the file is the returned
unit) with the matching section anchors.

```
vaire search <query> [--type <T>] [--scope <container-id>] [--limit <N>] [--local] [--json]
```

- `--type <T>` — restrict to nodes of a type.
- `--scope <container-id>` — restrict to nodes scoped under a container (matches the
  configured `scope_field`, default `scope`, §6.1). With `--scope`, every result is in that
  scope, so result `id`s are shown **scope-relative** — the node's own `type:id`, with the
  prefix omitted. Without `--scope`, scoped nodes show their full `<scope>/type:id`. (`path`
  is always the full package-root-relative path.)
- `--limit <N>` — max results (default: `10`).
- Ranking: FTS + aliases first, vectors for recall (design.md §9). Sorted by descending
  score; ties broken by (qualified) `id` ascending for determinism.
- **Cross-package** (§6.5): the query runs over this package **and its linked dependency
  closure** — that is the selective-consumption payoff: what you depend on is part of
  your knowledge. The query is embedded once and reused per member; a dependency indexed
  with different embedding dimensions contributes FTS/alias hits only (vector recall
  silently absent for it — `status` surfaces the mismatch). `--local` restricts to this
  package; `--scope @pkg/container` searches inside a dependency's container.
  Cross-package hits carry `package` in JSON; unavailable dependencies are listed in
  `skipped`.

JSON:

```json
{
  "query": "broker throughput",
  "results": [
    {
      "id": "record:2026-06-10-broker-sync",
      "type": "record",
      "path": "projects/atlas/2026_q2/meeting-notes/2026-06-10-broker-sync.md",
      "score": 0.82,
      "anchors": [
        { "heading": "Broker sync, 2026-06-10", "line": 9, "snippet": "raised throughput concerns about the broker" }
      ]
    }
  ],
  "count": 1
}
```

`anchors` point the caller at the relevant section(s); the caller opens the file at `line`
for depth. `score` is an opaque relative rank, not a calibrated probability.

### 3.5 `vaire unresolved`

Every unresolved reference (`[[?...]]`) currently in the corpus. This is the work list for
the entity-creation pass (design.md §8) and is derived fresh from the files on each call —
there is no stored queue.

```
vaire unresolved [--type <T>] [--scope <container-id>] [--all-packages] [--json]
```

- `--type <T>` — restrict to a `?type` hint (e.g. `--type person` matches `[[?person: …]]`;
  references written as `[[?: …]]` have type `null` and match only when `--type` is omitted).
- `--scope <container-id>` — restrict to records in a container.
- Sorted by `(source path, line)`.
- **Default scope: this package only.** A descriptor is package-agnostic and a
  dependency's loose ends are its owner's worklist (design.md §6 loose ends).
  `--all-packages` widens to the linked closure, rows tagged with their `package`
  (unavailable dependencies listed in `skipped`); it cannot combine with `--scope`.

JSON:

```json
{
  "unresolved": [
    {
      "record": "record:2026-06-10-broker-sync",
      "path": "projects/atlas/2026_q2/meeting-notes/2026-06-10-broker-sync.md",
      "type_guess": "person",
      "descriptor": "someone from logistics",
      "line": 9
    },
    {
      "record": "record:2026-06-10-broker-sync",
      "path": "projects/atlas/2026_q2/meeting-notes/2026-06-10-broker-sync.md",
      "type_guess": null,
      "descriptor": "the broker thing",
      "line": 9
    }
  ],
  "count": 2
}
```

### 3.6 `vaire render <id>`

Render a node as **portable Markdown**: its frontmatter kept verbatim, and its wikilinks
resolved to standard Markdown links (design.md §6). This is the one read command that
returns a file **body** rather than pointers — it does for the caller what they would
otherwise do by opening the file.

```
vaire render <id> [--json]
```

- Frontmatter is emitted unchanged.
- A resolved `[[type:id]]` becomes `[display](relative-path)`: `display` is the `|`
  override or the target's `name:` (falling back to the target slug); the href is a path
  to the target file **relative to this node's file**.
- An unresolved `[[?...]]` renders as its plain descriptor (it is not a link).
- A reference whose target is not a node (dangling), and any wikilink inside a fenced code
  block, are left verbatim.
- Follows `superseded_by` redirects when resolving link targets. Exit `5` if `<id>` is not
  a node.
- Cross-package (§6.5): `<id>` may be `@pkg/type:id` — the file is read from the linked
  package, and its inline references resolve in **that** package's context (its own
  `[dependencies]` and links). Same-package hrefs are unchanged; hrefs to another package
  are filesystem-relative through the links (`../../acme-core/….md`). JSON gains a
  `package` field for a cross-package node (absent for local ones).

Human output is the rendered Markdown itself. JSON:

```json
{
  "id": "record:2026-06-10-broker-sync",
  "path": "projects/atlas/2026_q2/meeting-notes/2026-06-10-broker-sync.md",
  "markdown": "---\nid: 2026-06-10-broker-sync\n...\n---\n# Broker sync, 2026-06-10\n\n[Jane Doe](../../../../knowledge/entities/people/jane-doe.md) walked ..."
}
```

### 3.7 `vaire suggest <descriptor>`

The **lookup-before-reference** primitive (design.md §7/§8): given a free-text descriptor
of something you want to reference, return ranked existing node IDs it might be. Use it to
turn a prose mention into an ID (then write `[[type:id]]`), or to confirm nothing matches
(then write `[[?type: descriptor]]`).

```
vaire suggest <descriptor> [--type <T>] [--limit <N>] [--local] [--json]
```

- Matches the descriptor against each node's `name`/`aliases` first (high precision), with
  prose full-text as a backup; no vectors (bare embeddings are weak for short descriptors,
  design.md §9). `--type <T>` restricts candidates to a type (the §8 type-gate). `--limit`
  default `5`.
- Sorted by descending score; ties broken by (qualified) `id` ascending.
- **Cross-package** (§6.5): candidates come from this package and its linked dependency
  closure — a dependency hit arrives pre-qualified (`@pkg/type:id`), ready to paste as a
  reference. `--local` restricts to this package; unavailable dependencies are listed in
  `skipped`.

JSON:

```json
{
  "descriptor": "logistics contact",
  "suggestions": [
    { "id": "department:logistics", "type": "department", "name": "Logistics",
      "path": "knowledge/entities/departments/logistics.md", "score": 3.5 }
  ],
  "count": 1
}
```

`score` is an opaque relative rank (an exact `name`/alias match outranks a token-subset
match, both outrank a prose-only hit).

### 3.8 `vaire deps`

The resolved local dependency tree — what each member's links actually point at.

```
vaire deps [--json]
```

- Pure **live link inspection** (§6.5): no index needed, so it is a safe first command in
  a fresh workspace. Always exits `0` — reporting is its job; erroring is `vaire check`'s.
- Each member's own dependencies resolve through *its* manifest and links (the same
  (source package, dependency name) keying as reference resolution). Cycles are annotated
  once (`(cycle)`) and not descended into; an unavailable dependency shows `MISSING` with
  the exact fix; a resolved version whose MAJOR falls outside the `^N` constraint is
  marked (surfaced only — enforcement is v0.3).

```
acme-web 1.0.0
├── acme-core ^1 → ../acme-core  (1.0.0)
│   └── acme-web ^1 → .  (1.0.0)  (cycle)
└── acme-shared ^1 → ../acme-shared  (1.0.0)
```

JSON is the nested tree: `{ "name", "version", "dependencies": [{ "name", "constraint",
"version", "resolved", "satisfied", "cycle"?, "note"?, "dependencies": […] }] }`.

## 4. Maintain commands

Not exposed over MCP. These read the working tree and write `.vaire/`; they never write the
corpus files. (`upgrade` is the exception with a different footprint: it touches only the
`vaire` binary itself.)

### 4.1 `vaire index`

(Re)build the index. The **source** and **mode** are chosen from the corpus's Git state:

```
vaire index [--full] [--working-tree] [--re-embed] [--no-deps]
```

- **Git repo with commits** → indexes the **committed** tree (commit-as-publish): the
  index state corresponds to exactly one commit. Default is **incremental** from the
  last-indexed commit (`git diff` the changed files, re-parse them, re-embed only changed
  sections — content-hash cache, design.md §9). This is the command a `post-commit` git
  hook calls. "Git repo" means the corpus root itself has `.git/`.
- **Not a Git repo, or no commits yet** → a full pass over the **working tree** read from
  disk, so a fresh or non-Git corpus (e.g. a corpus nested inside a larger repo) still
  indexes. The recorded commit is `null`; `status` reports it as such.
- `--full` — always a cold rebuild: drop and recreate `.vaire/index.db` and re-index
  everything from the applicable source.
- `--re-embed` — re-embed every section with the current provider, **bypassing the
  content-hash cache**, then repopulate it. Use after changing the embedding
  model/provider/`dimensions`: the cache is keyed by section text only, so a normal
  reindex would reuse the previous model's vectors for unchanged sections. Re-embeds from
  the already-indexed section bodies — no re-parse, no Git read — leaving nodes/edges and
  the commit anchor untouched. Requires an existing index (exit `4` otherwise).
- `--working-tree` — index the **working tree** (uncommitted edits) from disk regardless
  of Git state, for the edit→validate→commit loop. Always a full pass; the recorded commit
  is `null` (the index no longer corresponds to a commit). Opt-in — the default stays
  commit-as-publish. The index reflects the working tree until the next plain `vaire index`.
- **Restore invariant:** a plain `vaire index` (no `--working-tree`) always rebuilds from
  the committed tree and re-anchors to the last commit — it never builds incrementally on
  top of a working-tree index, so it cannot inherit uncommitted rows. (Internally the index
  records whether it is a `committed` or `working-tree` snapshot; incremental requires a
  prior committed one.)
- **Linked dependencies** (§6.5): a declared dependency that is not linked yet is first
  satisfied from the local-packages root when one is configured (§6.6) — reported as a
  `linked → <path>` line. Then each package in the linked closure gets its own index
  built/refreshed — with *its* manifest, repo, and
  commit anchor, written into *its* `.vaire/` (the federated model, design.md §9;
  incremental per dependency; a dependency embedded by a different provider is fully
  rebuilt so no index mixes vector spaces). One output line per dependency; an unlinked
  or broken dependency is a warning row, not a failure. `--no-deps` skips the pass
  (`--re-embed` is always current-package-only). The consumer records what its links
  resolved to (`deps_snapshot`) at index time.
- Writes only `.vaire/` dirs (the current package's, and linked dependencies' during the
  ensure pass — always derived caches, never any corpus).
- On completion prints a one-line summary (nodes, edges, sections embedded, elapsed) plus
  the per-dependency lines; `--json` emits the same as an object (with a `dependencies`
  array when the pass ran).
- An index whose **schema version** doesn't match this binary is rebuilt from scratch (the
  version is bumped on any schema change); a plain `vaire index` therefore self-migrates.
- Exit `3` if the index is structurally corrupt and cannot be opened (suggests `--full`).

### 4.2 `vaire check`

Run the integrity guards that ID-based discovery enables. Reads the index; exits non-zero
if any violation is found, so it works as a pre-commit hook or CI gate.

```
vaire check [--strict] [--working-tree] [--no-deps] [--json]
```

`--working-tree` reindexes from the working tree first (as `vaire index --working-tree`),
so the checks see uncommitted edits — the agent's edit→validate loop without a commit.

Checks:

- **Duplicate IDs** — two nodes sharing one `id:` (the duplicate-entity guard).
- **Dangling references** — a non-`?` reference whose target is not a node: local, or —
  since M5 — cross-package (`@pkg/type:id` whose target, after tombstone-following in the
  owning package's context, does not exist). Existence-based, so the dependency's own
  `types` vocabulary is irrelevant.
- **Undeclared import** — an `@pkg/…` reference whose package is not in the manifest
  `[dependencies]` (packages.md §8). A pure table check — no cross-package resolution needed.
- **Missing dependency** — a *declared* dependency that is unavailable (not linked,
  broken link, name mismatch — cli.md §6.5). Reported **once per dependency** with the
  exact fix; its edges are skipped by the dangling pass (no spam).
- **Frontmatter/inline drift** — a resolved reference linked **inline** whose target is
  not also in the frontmatter edge list (the actionable "declare it" direction). Advisory,
  since narrative inline links legitimately exceed the structured edge list — a *warning*,
  not a failure.
- **Orphans** — nodes with no inbound or outbound edges. A warning.
- **Unreferenceable id** — a node whose declared `id`/scope falls outside the reference
  grammar (design.md §6), so nothing can address it. The node still indexes. A warning.
- **Unused dependency** — declared in `[dependencies]` but never referenced. A warning.
- **Version mismatch** — a linked dependency whose declared MAJOR falls outside this
  package's `^N`. A warning — *surfaced only*; version enforcement is explicitly v0.3.

Duplicate IDs, dangling references, undeclared imports, and missing dependencies are
violations. The rest are warnings; `--strict` promotes them to failures. Exit `0` clean,
`6` on any violation (or any warning under `--strict`).

With linked dependencies the run starts with the same **ensure pass** as `vaire index`
(near-no-op when fresh; `--no-deps` skips it), so the resolution lints judge commit-fresh
dependency indexes — a cold clone can run `vaire check` first. The dependency *cycle*
between packages terminates structurally: the lints iterate this package's edge table plus
point lookups, never a graph traversal.

JSON:

```json
{
  "ok": false,
  "violations": [
    { "kind": "dangling_ref", "from": "record:2026-06-10-broker-sync", "to": "system:ingestt-api", "path": "projects/atlas/2026_q2/meeting-notes/2026-06-10-broker-sync.md", "line": 6 }
  ],
  "warnings": [
    { "kind": "orphan", "id": "method:legacy-thing", "path": "knowledge/entities/methods/legacy-thing.md" }
  ]
}
```

`kind` is one of `duplicate_id`, `dangling_ref`, `undeclared_import`, `missing_dependency`
(violations), `drift`, `orphan`, `frontmatter_wikilink`, `unknown_type`,
`unreferenceable_id`, `scoped_type_not_permitted`, `unused_dependency`,
`dependency_version_mismatch` (warnings). `unknown_type` flags a frontmatter value that matches the reference `target`
grammar (`field: team:alpha`) whose type isn't in `types` — it was *ignored* rather than
made an edge, so the warning surfaces the silent drop (declare the type, or quote the value
as a string). Identification is by shape (design.md §6), so a URL, a time, or a colon in a
non-reference value (`url: https://…`, `summary: "TODO: …"`) is structurally not a
reference and is never flagged. `unreferenceable_id` flags a node whose *declared* id or
scope falls outside the target grammar (`id: Jane_Doe`) — the file still indexes (files are
truth), but no reference can ever address it, so the trap is surfaced instead of silent.

### 4.2a `vaire add`

Declare a dependency on another package — and, with `--link`, wire up where it lives.

```
vaire add <name>[@^MAJOR] [--link <path>]
# e.g. vaire add acme-core --link ../acme-core
#      vaire add acme-web@^2
```

Edits `[dependencies]` in `knowledge.toml`, **preserving the file's formatting and
comments** (the manifest is authored, not generated). `^MAJOR` is the only legal constraint
(manifest.md §5); the default is `^1`. Adding a package that is already present updates its
constraint in place — idempotent. A malformed name or constraint is a usage error (exit `2`),
caught before the manifest is touched. Needs the package root but not the index.

Declaring is the whole job; **wiring is a convenience on top** and never fails the
command. Without `--link`, the dependency is satisfied from the local-packages root if it
can be found there (§6.6) — the JSON then carries `"discovered": true`. If it cannot (no
root configured, nothing declaring that name, or an ambiguous name), the dependency is
declared but unlinked — a first-class state, reported as a `note` — and `vaire index` will
try again.

`--link <path>` instead wires it explicitly, creating (or replacing) the
**`.vaire/packages/<name>`** symlink pointing at `<path>` (§6.5). The target must be a
package whose `knowledge.toml` declares that same `name` (usage error otherwise — identity
is declared, never path-derived; a bad `--link` leaves the manifest untouched). An explicit
link always wins over discovery. The manifest never carries the path either way: the
committed contract stays machine-independent, the link is per-checkout state under
gitignored `.vaire/`.

```json
{ "name": "acme-core", "constraint": "^1", "config_path": "knowledge.toml", "updated": false,
  "linked": "/Users/you/Documents/Knowledge/acme-core", "discovered": true }
```

### 4.3 `vaire status`

Report index state.

```
vaire status [--json]
```

Human:

```
repo:            /Users/example/corpus
index:           .vaire/index.db
last-indexed:    a1b2c3d  (3 commits behind HEAD)
nodes:           412   (people 38, departments 9, records 351, …)
edges:           1.9k
embeddings:      cached 1180 / 1190 sections
dependencies:
  acme-core    fresh  142 nodes  def5678 (up to date)
  acme-shared  missing — dependency 'acme-shared' is not linked — run `vaire add …`
```

With linked dependencies (§6.5), one row per closure member reports its state: `fresh`,
`stale-schema`, `missing`, or `unreadable`; its own last-indexed commit and lag; and its
embedding provider — when a dependency's provider differs from this package's, the row
warns that vector search silently skips it (FTS/alias hits still work). Status stays
tolerant: an unlinked or broken dependency is a reported row, never a failure. JSON gains
`embed_provider` and a `dependencies` array.

JSON:

```json
{
  "repo": "/Users/example/corpus",
  "index_path": ".vaire/index.db",
  "schema_version": 1,
  "source": "committed",
  "last_indexed_commit": "a1b2c3d",
  "commits_behind_head": 3,
  "nodes": { "total": 412, "by_type": { "person": 38, "department": 9, "record": 351 } },
  "edges": 1903,
  "embeddings": { "sections": 1190, "cached": 1180 }
}
```

`source` distinguishes the three states, so a populated index never reads as "not built":

- `"committed"` — built from a commit; `last_indexed_commit` is set and `commits_behind_head`
  > 0 means the working tree has commits the index has not absorbed (run `vaire index`).
- `"working-tree"` — built from uncommitted edits (`vaire index --working-tree`);
  `last_indexed_commit` is `null` and `last-indexed` shows **"working tree (uncommitted)"**,
  *not* "not built yet".
- `null` — no index. `status` is the one read-adjacent command that tolerates this, showing
  "not built yet" and exiting `0`.

### 4.4 `vaire init`

Scaffold a corpus so it becomes discoverable. Discovery keys off a `.vaire/` directory
(§2.1), so a brand-new corpus needs one before any other command can find it. `init` is the
exception that does **not** use discovery — it is what makes the repo discoverable.

```
vaire init [path]
```

- `path` — directory to initialize. If omitted, `--repo`/`VAIRE_REPO` is used as the target;
  if neither is given, the current directory. An explicit `path` takes precedence over
  `--repo`/`VAIRE_REPO`. Created if absent.
- Writes `<path>/knowledge.toml` (the committed corpus marker, with the §6 defaults) and a
  self-contained `<path>/.vaire/.gitignore` that ignores everything derived under `.vaire/` —
  so `init` need not touch the repo's root `.gitignore`.
- **Migration:** if a legacy `<path>/.vaire/config.toml` exists, it is migrated into
  `knowledge.toml` (renaming `id_prefixes` → `types`, converting a non-empty `scoped_types`
  into `scoped_types_whitelist`, dropping `[embeddings]`, injecting `name`/`version`) and set
  aside as `.vaire/config.toml.migrated`.
- Does **not** create a Git repo or build the index; it prints the next step (`vaire index`).
- Exit `2` if the directory is already a corpus (`knowledge.toml` exists) — `init`
  never clobbers an existing manifest.

### 4.5 `vaire upgrade`

Self-update: replace the running binary with a released one. Corpus-independent (no
discovery — it operates on the executable, not a package), and follows the **same
contract as the installer scripts**: resolve the tag from the GitHub releases API,
download the `vaire-<tag>-<target-triple>` asset for the triple this binary was
compiled for, extract it with the system `tar`, and atomically swap it over the
current executable.

```
vaire upgrade [<version>] [--check] [--json]
```

- Versions are **bare** everywhere the user sees them (`0.3.0`); the `v` prefix exists
  only on the underlying git tag and the URLs built from it (a leading `v` is accepted
  on input).
- Without arguments: resolve the latest release and install it only when the semver
  comparison says it is strictly **higher** than this build. Same version → "up to
  date", exit `0`; a build *ahead* of the newest release (e.g. built from source before
  the tag is cut) is never downgraded; versions that don't compare as
  `MAJOR.MINOR.PATCH` refuse with a hint to pin explicitly rather than guess.
- `<version>` (e.g. `0.3.0`) pins the release to install — and an explicit version
  **always** installs, even the currently running one, which is how a corrupted
  install is repaired in place. Downgrading is allowed only this way, explicitly.
- `--check` reports what would happen (current vs latest, for this target triple) and
  installs nothing.
- The swap is atomic: the new binary is staged next to the executable (same
  filesystem) and `rename`d over it, so the install is never half-written. On Windows
  the running `.exe` is moved aside first (a leftover `vaire.exe.old` may remain until
  the next upgrade removes it).
- **Package-manager guard:** when the binary lives in a location a package manager
  owns (`~/.cargo/bin`, a Homebrew Cellar, the Nix store), `upgrade` refuses and
  prints that manager's own upgrade command. Vairë is not distributed through any
  package manager yet; when it is, upgrades are that manager's job and this guard is
  the seam that keeps self-update from fighting it.
- Failures (API unreachable, no prebuilt asset for this platform, no write access to
  the install directory) exit `1` with kind `upgrade`; the platform-asset message
  points at `cargo install --path .` as the fallback, like `install.sh` does.

JSON (`installed` and `note` appear only when set):

```json
{
  "current": "0.2.0",
  "latest": "0.3.0",
  "target": "aarch64-apple-darwin",
  "up_to_date": false,
  "checked_only": false,
  "installed": "/home/user/.local/bin/vaire"
}
```

### 4.6 `vaire pack`

```text
vaire pack [--no-embeddings] [--json]
```

Build this package's distributable artifact: `.vaire/dist/<name>-<version>.tgz`, a
gzipped tar with a single top-level directory `<name>-<version>/` holding the manifest,
every corpus file the manifest's include/exclude selects, **every file those reference**
by relative link or image (transitively through referenced Markdown), and a freshly
exported `.vaire/index.db`. This is the unit a registry stores and a consumer pulls
(design: the project registry doc, v0.2).

- **The committed tree is the only input.** Packing is commit-as-publish taken
  literally: the corpus root must be a Git repository with commits (a corpus nested in a
  larger repo indexes from the working tree but cannot honestly claim "this artifact is
  commit X", so it cannot pack), and `knowledge.toml` must be committed and identical to
  the working copy — the manifest is the artifact's identity *and* its file-selection
  rules. A dirty working tree otherwise warns and packs HEAD.
- **Publication gate.** `pack` refreshes the index to HEAD and runs the `vaire check`
  suite first; violations refuse the pack (exit `6`), because an artifact with
  violations ships broken references to every consumer.
- **Attachments ship by reference.** Every relative link/image target — inline
  `[]()`/`![]()` and reference-style `[label]: path` definitions, fenced code skipped —
  reachable from a packed `.md` is pulled into the artifact, transitively through
  referenced Markdown (a shipped document never carries broken links of its own). There
  is no reserved directory and no extension list: the reference is the declaration, and
  an unreferenced file simply does not ship (orphans cannot exist). Referenced payload
  is never corpus — the include/exclude globs alone decide what gets an id. Three
  author decisions outrank a link: an **exclude glob vetoes** shipment (warning — a
  stray link must not republish a draft); a **gitignored** target is declared
  local-only (warning); anything else **missing at HEAD** or escaping the package root
  fails the pack (exit `1`, kind `pack`) — a typo or a forgotten `git add`. A
  trailing-`/` target is a directory link: satisfied by any shipped file under it,
  never an inclusion demand. URLs, `mailto:`, bare `#anchors`, and HTML `<img>` are out
  of scope; wikilinks belong to `check`.
- **The shipped index is exported, not copied**: a fresh database written in a fixed
  order. The machine-local `embed_cache` never ships; `deps_snapshot` is rewritten to
  `{name, version, constraint}` (an artifact records choices, never locations);
  `packed_by` records the packing vaire. The FTS structure does **not** ship (its
  segments embed random identity) — it is derived state over the shipped sections,
  recreated when the artifact is materialized into a store. Section vectors ship by
  default with their `embed_provider` identity; `--no-embeddings` strips them.
- **Reproducible**: entries sorted, timestamps pinned to the commit, ownership zeroed,
  gzip untimestamped, compression level fixed. Same commit, same flags, same bytes —
  with vectors included this additionally requires the same vectors (the content-hash
  cache makes repeat packs on one machine stable); `--no-embeddings` is unconditional.

JSON:

```json
{
  "name": "acme-core",
  "version": "1.4.0",
  "commit": "8c0f2a…",
  "artifact": ".vaire/dist/acme-core-1.4.0.tgz",
  "sha256": "ab12…",
  "size_bytes": 148215,
  "entries": 61,
  "nodes": 58,
  "embeddings": 214,
  "warnings": ["knowledge/pointer.md:12 links to drafts/wip.md, which the exclude globs veto; it stays out of the artifact"]
}
```

### 4.7 `vaire release`

```text
vaire release [--major] [--dry-run] [--notes <file>] [--allow-branch] [--yes] [--json]
```

Cut a release: classify what changed since the last one, compute the version, write the
manifest and a release record, commit, tag. **The user never types a version number** —
except to declare a major dependency, or to deliberately cut a major.

- **The bump is computed, not typed.** The classifier diffs the entity index of the last
  release against the current tree, rebuilding the baseline from that release's tag (a
  pack is byte-deterministic from a commit, so nothing needs the old artifact retained):

  | observed                                                  | bump               |
  |-----------------------------------------------------------|--------------------|
  | entity addresses added; none removed or retired            | **MINOR**          |
  | content changed, address set identical                     | **PATCH**          |
  | addresses removed, or a `superseded_by:` appeared          | **MAJOR** — gated  |
  | mixed                                                      | highest applicable |

  "Content" is what a reader notices — sections, edges, aliases. A moved file or a
  touched `updated:` is bookkeeping and never a release. A **rename** needs no special
  case: an address *is* the identity, so a rename is one removal plus one addition, and
  the removal already forces MAJOR. A package with **no prior tag** publishes the version
  its manifest already declares — a first release is a declaration, not an increment.
- **MAJOR is never automatic** (exit `7`, distinct so a pipeline can report *pending*
  rather than *broken*). It needs `--major` **and** `--notes <file>`, the human-written
  invalidated assumptions dependents read to decide about re-confirmation. `--major` also
  *escalates*: a one-word edit reversing a truth is a semantic act the classifier cannot
  see, and the maintainer owns meaning while the tool owns structure.
- **The release record is the changelog, written as corpus.** `releases/<version>.md`, an
  entity carrying the date, the bump, and **edges** to what was added, changed, and
  retired — so "which releases touched this entity?" is `vaire backlinks <id> --type
  release`, and a consumer's adopted-changes digest is an intersection rather than a diff.
  Removed addresses are recorded as text, not references: a deleted entity has no address
  left to point at. The type and directory are `release_type`/`release_dir` in the
  manifest — conventions, not reserved words. **The classifier excludes release records
  from its own diff**, or no release after the first could ever be a PATCH.
- **Gates.** A clean working tree (the release commit contains the release and nothing
  else — the `--notes` file is an input, not stray work), HEAD on the repository's
  mainline (`--allow-branch` escapes; a tag cut on a topic branch names a commit the
  mainline may never contain), the package as its own Git repository, a version whose tag
  does not already exist, a record path the include globs actually select, and `vaire
  check` free of violations — a published version with dangling references ships them to
  every consumer. Warnings are reported, never fatal.
- **Nothing to release is a clean no-op, exit `0`.** An automated pipeline runs this on
  every merge and most merges warrant no version.
- **It never runs `git push`, and never uploads.** Git transport stays yours; publishing
  to a registry is `vaire push` (not yet implemented) — so a flaky upload re-runs an
  upload rather than a ritual, and CI can publish a tag it did not cut. `--push` and
  `--onto` are reserved grammar, rejected as not yet implemented.

`vaire status` reports the same classification ambiently (`release: would be minor — 3
new, 12 changed`), so a release is never a surprise.

JSON:

```json
{
  "package": "acme-core",
  "status": "released",
  "version": "1.5.0",
  "bump": "minor",
  "outcome": {"kind": "bump", "bump": "minor"},
  "added": ["system:ingest"],
  "changed": ["department:platform"],
  "retired": [],
  "removed": [],
  "tag": "v1.5.0",
  "record": "releases/1-5-0.md",
  "commit": "d8ecc979227…",
  "warnings": 3,
  "advisories": [{ "id": "department:platform", "inbound": 14 }]
}
```

`advisories` carries the backlink weighting — changed entities with ten or more inbound
references, present on `planned` and `released` alike, and absent when there are none.

`status` is one of `released`, `planned` (`--dry-run`), `nothing`, or `blocked` (a MAJOR
awaiting a maintainer, exit `7`).

## 5. MCP server

```
vaire mcp [--repo <path>]
```

Starts a Model Context Protocol server over STDIO. It exposes the **read** commands as
MCP tools, one-to-one:

| MCP tool | CLI equivalent |
| --- | --- |
| `resolve` | `vaire resolve <id>` |
| `render` | `vaire render <id>` |
| `backlinks` | `vaire backlinks <id>` |
| `refs` | `vaire refs <id>` |
| `search` | `vaire search <query>` |
| `suggest` | `vaire suggest <descriptor>` |
| `unresolved` | `vaire unresolved` |
| `deps` | `vaire deps` |

- Tool input schemas mirror each command's args and flags; tool results are the command's
  `--json` shape (§3) verbatim. There is no second serialization to maintain.
- Maintenance commands (`index`, `check`, `status`) are **not** exposed — the agent-facing
  surface is bounded to reads (design.md §9).
- The server operates against the already-built index and never builds or writes it. If the
  index is missing, tool calls return an MCP error pointing at `vaire index` (mirroring exit
  `4`); the agent is not allowed to trigger a build.
- One server instance serves one repo, resolved at startup via the same discovery as the CLI
  (§2.1).

## 6. Configuration

Authored config lives in the committed **`knowledge.toml`** at the package root — the package
manifest, specified in full in [`manifest.md`](manifest.md). Everything under `.vaire/` is
derived and gitignored (design.md §9). All keys except `name`/`version` are optional; defaults
make `vaire` work from a two-line manifest. The settings that affect the command surface:

```toml
# knowledge.toml — committed package manifest (full spec: manifest.md)

name    = "my-package"   # required: package id (slug)
version = "0.1.0"        # required: semver

# Where to look (an `id:`+`type:` pair is still what makes a file a node; these only
# bound the search space).
include = ["knowledge/**/*.md", "projects/**/*.md"]
exclude = ["**/node_modules/**", "**/drafts/**", "**/archive/**"]

# Type vocabulary — the `type:` field / ID prefix in `type:id`. The types this package
# defines. Load-bearing for *classification* (design.md §6): a frontmatter value that
# matches the reference target grammar becomes an edge only when its type is listed here;
# a matching value with an unlisted type is surfaced by `vaire check` (unknown_type).
# Identification is by shape, not by this list — URLs, titles, and notes are structurally
# not references and are left alone.
types = ["person", "department", "method", "system", "event", "record", "project"]
vocabulary_strict = false

# Scoped IDs. Scoping is DATA-DRIVEN: any node carrying the `scope_field` gets the composed
# address `<container-id>/<type>:<local-id>` (e.g. project:atlas-2026-q2/record:2026-06-10-standup),
# regardless of type. The two lists are a LINT POLICY only (not a gate): `vaire check` warns
# when a scoped node's type is not permitted. `"*"` matches any type; defaults permit all. §6.1.
scoped_types_whitelist = ["*"]
scoped_types_blacklist = []
scope_field  = "scope"      # frontmatter field that supplies the scope; its value names the
                            # container (scope: project:atlas, scope: org:some-firm, …)

# Dependencies on other packages: `name -> "^MAJOR"` (the only legal constraint form).
[dependencies]
# other-package = "^1"
```

Resolution order for any setting: `--config` path > `<root>/knowledge.toml` > built-in
defaults.

**Embeddings** are a machine/consumer choice, not part of the package contract, so they are
**not** manifest settings — see `manifest.md` §6. The providers themselves (`local` built-in,
`command` shelling out via `sh -c`, `openai` via the API) are unchanged; the API key resolves
as in §6.3.

### 6.1 Scoped IDs

Without scoping a record needs a globally-unique slug — projects end up hand-prefixing
(`record:nova-2026-06-10-standup`), re-deciding the prefix per project. Scoping removes that tax.

Scoping is **data-driven**: whenever a node carries the `scope_field` (default `scope`), its
address is a **path of typed IDs** — `<container-id>/<type>:<local-id>` — regardless of type.
(The `scoped_types_whitelist`/`scoped_types_blacklist` settings are a lint policy over which
types *should* be scoped, §6; they do not gate this behaviour.)

```markdown
# a record writes only its local id; scope: names the container
---
id: 2026-06-10-standup
type: record
scope: project:atlas-2026-q2
---
```
⇒ address `project:atlas-2026-q2/record:2026-06-10-standup`.

- The node's own ID is the last segment (`record:2026-06-10-standup`), and `type:` filters
  / counts use *that* type (`record`). The `project:atlas-2026-q2/` prefix is the scope.
- `<local-id>` (the `id:` field) need only be unique **within its container**.
- A node's own scope is **purely local** — it is the node's own `scope_field` value, so no
  per-container declaration is needed. The container's ID *is* the scope, so uniqueness and
  stability come for free (its ID is unique and doesn't get renamed in place).
- **The container can be any type.** The type lives in the field's value, so the same field
  scopes under different containers: `scope: project:atlas-2026-q2` →
  `project:atlas-2026-q2/record:…`, `scope: org:some-firm` → `org:some-firm/record:…`. Only
  the *field name* is configured (`scope_field`, default `scope`; e.g. set to `project` to
  tie scoping to a specific relationship field).
- The `scope:` value is also a graph edge (`ref_type: scope`) to the container — so the
  container should be a real node, or `vaire check` reports a `dangling_ref`.

**References:**

- Full anywhere: `[[project:atlas-2026-q2/record:2026-06-10-standup]]`.
- **Relative**, resolved **scope-first then global**: a bare `[[type:id]]` inside a scoped node
  resolves to `<container>/type:id` when that sibling exists, otherwise to the global `type:id`.
  So `[[record:2026-06-10-standup]]` finds the same-container record, while `[[person:jane]]`
  (no scoped sibling) resolves globally. Existence-based, so it needs no type list.

**Entities stay global** — no `project:`, no scope; the entity/record split is expressed in
the ID. Nesting is one level (project) today; the grammar (a `/`-separated path of typed
segments) leaves room for deeper containers later.

### 6.2 Frontmatter references (and the `[[ ]]` trap)

Frontmatter references are **bare** — the `[[ ]]` brackets are an inline-prose convention,
not a frontmatter one. A frontmatter field value is interpreted in **two steps**
(design.md §6): *identification* — does the value match the reference `target` grammar? —
which is purely syntactic and config-free, then *classification* against the manifest
`types`:

- a value matching `target` whose type is in `types` → a resolved **edge** keyed by the
  field (`org: department:platform`, `head: person:jane-doe`).
- a value matching `target` whose type is **not** in `types` → **no edge**, and
  `vaire check` warns (`unknown_type`) — the ambiguity is surfaced, never silently
  swallowed: quote the value as a string, or declare the type.
- a value that does not match `target` → a plain scalar, full stop. The strict charset
  means URLs, emails, times, and dates (`url: https://…`, `summary: "TODO: …"`) are
  structurally not references — no config consulted. (`name` and `aliases` are display
  fields and are never scanned for references.)
- `"?type: descriptor"` / `"?: descriptor"` → an **unresolved** loose end (§6), surfaced by
  `vaire unresolved` — quote it because of the leading `?`. This lets a structured field
  name something not yet created: `head: "?person: someone senior"`. It is *not* an edge.

**The trap:** writing an inline-style `[[ ]]` in frontmatter (`head: [[?person: Foo]]`) is
a natural muscle-memory mistake. Unquoted, YAML parses it to nested junk (a silent no-op);
quoted, it is a meaningless string. Vairë forgivingly **strips** stray surrounding brackets
(so `head: "[[person:jane]]"` still links), but `vaire check` always **warns**
(`frontmatter_wikilink`) so the mistake surfaces rather than failing silently. The fix is
to drop the brackets: `head: person:jane` (resolved) or `head: "?person: …"` (unresolved).

### 6.3 Secrets — `credentials.toml`

Providers that need credentials (currently `openai`) resolve them with the precedence
**environment variable first, then `<config-home>/credentials.toml`** (the user config home,
§6.4):

- `OPENAI_API_KEY` — required for `provider = "openai"`. Set it in the shell environment, or
  pipe it to `vaire configure embeddings --api-key-stdin` (which writes
  `credentials.toml` without putting the key in shell history or process arguments).
- `OPENAI_BASE_URL` — optional; overrides the API endpoint (proxies / Azure-style gateways).
  It must use HTTPS, except for an explicit loopback development endpoint.

`credentials.toml` is a TOML `KEY = "value"` table written with `600` permissions (owner
read/write only) on Unix. It lives in the user config home, never in a corpus, so secrets are
never committed. `vaire` reads it on demand — it does not mutate the process environment.

### 6.4 Global user config — `vaire configure`

Machine/consumer settings are **not** part of any package manifest (a package must not dictate
how a consumer indexes it). They live in a per-user config, set with `vaire configure`. The
command has two forms:

```text
vaire configure                       # interactive: pick a section, then guided prompts
vaire configure embeddings [--provider local|command|openai] [--model <m>]
                           [--dimensions <n>] [--command <cmd>]
                           [--api-key-stdin] [--api-url <url>]
vaire configure local-packages [<path>] [--unset]
```

- Bare `vaire configure` opens an interactive prompt (sections: **Embeddings**, **Local
  packages**). Cancelling (Esc / Ctrl-C) exits cleanly and writes nothing.
- `vaire configure embeddings` sets the same settings non-interactively.
- `vaire configure local-packages <path>` records where your local packages live (§6.6);
  with no argument it reports the current setting, `--unset` clears it. The path is stored
  canonical, and must exist (a typo is rejected here rather than surfacing later as a
  dependency that cannot be found).
- Non-secret settings (embedding provider, model, dimensions, command, local-packages
  root) → `config.toml`.
- Credentials (`--api-key-stdin`, `--api-url`) → `credentials.toml` (§6.3).
- Only the flags you pass are changed; the rest are preserved.

```toml
# <config-home>/config.toml
[embeddings]
provider = "local"
dimensions = 384

[packages]
local = "/Users/you/Documents/Knowledge"
```

The **config home** is `VAIRE_CONFIG_HOME` if set, else the platform config directory for
`vaire` (`~/.config/vaire` on Linux, `~/Library/Application Support/vaire` on macOS,
`%APPDATA%\vaire` on Windows).

### 6.5 Linked packages — `.vaire/packages/`

Where a declared dependency **lives** on this machine. Each entry
`.vaire/packages/<name>` is a symlink (or a real directory) whose target is a package: a
directory with a `knowledge.toml` declaring that same `name`. `vaire add <name> --link
<path>` creates the entry (§4.2a); everything under `.vaire/` is gitignored, so links are
per-checkout state — the committed manifest carries only `name = "^MAJOR"`.

**Resolution.** An `@pkg/type:id` reference resolves through the *referencing* package's
own dependencies (design.md §9): the alias must be in that package's `[dependencies]`
(else `undeclared_import`), then the target package is found at

1. the referencing package's own `.vaire/packages/<name>`, else
2. the **run-root package itself**, when the name is the run-root's declared name — a
   dependency cycle back into the package you're standing in needs no link, else
3. the **run-root** package's `.vaire/packages/<name>` (the package the command was
   invoked from) — the fallback that lets one flat set of links at your top level serve
   the whole transitive closure.

Own links win; the fallback is a convenience. Failure modes are specific and actionable:
**declared but not linked** ("run `vaire add <name> --link <path>`"), **broken link**
(target missing), **name mismatch** (target declares a different `name`), and an invalid
target manifest (parse error attached as a note). A package cannot depend on itself —
bare references are already local.

The layout is the forward-compatible seam: a future `vaire install` will populate the
same entries as links into a shared local cache resolved through a lockfile, changing
nothing about how references resolve. Linked-dependency indexes live inside each linked
package's own `.vaire/` (design.md §9, federated index) — `vaire index` refreshes them
through the link; reads never build.

### 6.6 Local packages — satisfying a declared dependency

A manifest declares *what* a package depends on. **Where** that dependency lives is a
property of this machine — one clone here, another there — so it is a user setting, never
a manifest key. Point Vairë at the directory your packages live in:

```bash
vaire configure local-packages ~/Documents/Knowledge     # §6.4
```

With that set, a declared dependency with no `.vaire/packages/<name>` entry is satisfied
automatically: the root is searched for a package **declaring** that name, and the link
(§6.5) is materialized. A fresh clone needs no wiring step —

```bash
git clone git@github.com:acme/acme-web && cd acme-web
vaire index          # links every declared dependency it can find, then builds
```

The rules:

- **Matching is by declared name, at any depth.** Directory names are irrelevant, so a
  knowledge base that is one component of a larger repo
  (`~/Documents/Knowledge/platform-docs/docs/kb` declaring `acme-handbook`) is found like
  any other package. The walk skips dotted directories and the usual build/vendor trees,
  does not descend into a directory that is already a package, and is depth-capped.
- **Ambiguity is reported, never guessed.** If two packages under the root declare one
  name (a fork beside its original), the dependency stays unsatisfied and both paths are
  named — link the one you want explicitly.
- **Only gaps are filled.** An existing, resolvable entry is never rewritten, so an
  explicit `--link` always wins. A *broken* entry is re-discovered, healing a package that
  moved or was renamed.
- **Links land in the run-root's `.vaire/packages/`**, never inside a dependency's
  directory — a transitive dependency is resolved from there by the run-root fallback
  (§6.5).
- **Only commands that already write links discover**: `vaire add`, and the ensure pass of
  `vaire index` / `vaire check`. A read command can never materialize a link.
- Nothing here is fatal. An unsatisfiable name keeps its ordinary "not linked" reporting,
  with a note saying what the root held. Unset (the default), no discovery happens at all.

This is also the v0.3 seam: `vaire install` will populate the same entries from a fetched
cache, and the same by-declared-name search is what locates a package inside a repository
fetched from a registry or git remote.

## 7. Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success. For `check`: no violations. |
| `1` | Generic/unexpected error. |
| `2` | Usage error — bad flags or arguments (also what `--help` paths use). |
| `3` | Index unreadable/corrupt, or its schema version doesn't match — rebuild with `vaire index --full`. |
| `4` | No corpus repo found, or index not built yet (read commands). |
| `5` | ID not found (`resolve`, `backlinks`, `refs` on a non-existent node). |
| `6` | `vaire check` found violations (or warnings under `--strict`). |

With `--json`, every non-zero exit also writes a JSON error to stdout so machine callers
parse one shape:

```json
{ "error": { "code": 5, "kind": "id_not_found", "message": "no node with id 'person:nobody'" } }
```

## 8. Examples

```bash
# Authoring: look up before referencing (design.md §7)
vaire search "logistics department" --type department --limit 5
vaire resolve department:logistics

# Reading the graph
vaire backlinks person:jane-doe --type record
vaire refs record:2026-06-10-broker-sync --depth 2

# The entity-creation pass's work list
vaire unresolved --type person --json | jq '.unresolved[].descriptor'

# Maintenance (humans / hooks / CI)
vaire index                 # incremental, e.g. from a post-commit hook
vaire check --strict        # e.g. a pre-commit / CI gate
vaire status

# Agents
vaire mcp                   # STDIO MCP server exposing the read tools
```
