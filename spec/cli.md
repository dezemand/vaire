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
- **Maintain** — `init`, `index`, `check`, `status`. Scaffold, build, validate, and report.
  **Not** exposed over MCP; run by humans, git hooks, or CI.

A read command run before the index exists is an error directing the user to `vaire index`
(exit `4`, see §7) — `vaire` never silently builds the index as a side effect of a query,
because indexing is bound to commit (commit-as-publish) and should be deliberate.

## 2. Global conventions

### 2.1 Repo discovery

`vaire` operates on one corpus repository. It locates the root by walking up from the
working directory to the nearest directory containing a **`.vaire/`** directory — the
committed `.vaire/config.toml` is what marks a directory as a corpus (run `vaire init` to
create one, §4.4). The index lives at `<root>/.vaire/index.db`.

Override with `--repo <path>` or the `VAIRE_REPO` environment variable (`--repo` wins); an
explicit path that has no `.vaire/` is an error rather than a silent guess. If no corpus is
found and none is given, exit `4`.

Discovery is deliberately decoupled from Git: the corpus root need not be a Git repo root.
Whether the index is built from the committed tree or the working tree is a separate
question, decided by `vaire index` from the corpus's Git state (§4.1).

### 2.2 Global flags

| Flag | Meaning |
| --- | --- |
| `--repo <path>` | Corpus repo root. Overrides discovery and `VAIRE_REPO`. |
| `--json` | Emit JSON instead of human-readable text. Read commands only. |
| `--config <path>` | Path to the config file (default: `<root>/.vaire/config.toml`, see §6). |
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

- `<id>` — a composed node ID `type:id`, e.g. `person:jane-doe`, `department:hr`.
- Follows `superseded_by` redirects (design.md §8) and reports the redirect in the result.
- `type` is the node's `type:` field; `frontmatter.name` is the reference's default
  display text — what `[[type:id]]` renders as (design.md §6).
- Exit `5` if the ID is not a node.

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
- Sorted by referencing node `id` ascending.

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
vaire search <query> [--type <T>] [--scope <project-id>] [--limit <N>] [--json]
```

- `--type <T>` — restrict to nodes of a type.
- `--scope <container-id>` — restrict to nodes scoped under a container (matches the
  configured `scope_field`, default `scope`, §6.1). With `--scope`, every result is in that
  scope, so result `id`s are shown **scope-relative** — the node's own `type:id`, with the
  prefix omitted. Without `--scope`, scoped nodes show their full `<scope>/type:id`. (`path`
  is always the full repo-relative path.)
- `--limit <N>` — max results (default: `10`).
- Ranking: FTS + aliases first, vectors for recall (design.md §9). Sorted by descending
  score; ties broken by `id` ascending for determinism.

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
vaire unresolved [--type <T>] [--scope <project-id>] [--json]
```

- `--type <T>` — restrict to a `?type` hint (e.g. `--type person` matches `[[?person: …]]`;
  references written as `[[?: …]]` have type `null` and match only when `--type` is omitted).
- `--scope <project-id>` — restrict to records in a project.
- Sorted by `(source path, line)`.

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
vaire suggest <descriptor> [--type <T>] [--limit <N>] [--json]
```

- Matches the descriptor against each node's `name`/`aliases` first (high precision), with
  prose full-text as a backup; no vectors (bare embeddings are weak for short descriptors,
  design.md §9). `--type <T>` restricts candidates to a type (the §8 type-gate). `--limit`
  default `5`.
- Sorted by descending score; ties broken by `id` ascending.

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

## 4. Maintain commands

Not exposed over MCP. These read the working tree and write `.vaire/`; they never write the
corpus files.

### 4.1 `vaire index`

(Re)build the index. The **source** and **mode** are chosen from the corpus's Git state:

```
vaire index [--full] [--working-tree] [--re-embed]
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
- Writes only `.vaire/` (creates `index.db` inside the existing `.vaire/`); never the corpus.
- On completion prints a one-line summary (nodes, edges, sections embedded, elapsed);
  `--json` emits the same as an object.
- An index whose **schema version** doesn't match this binary is rebuilt from scratch (the
  version is bumped on any schema change); a plain `vaire index` therefore self-migrates.
- Exit `3` if the index is structurally corrupt and cannot be opened (suggests `--full`).

### 4.2 `vaire check`

Run the integrity guards that ID-based discovery enables. Reads the index; exits non-zero
if any violation is found, so it works as a pre-commit hook or CI gate.

```
vaire check [--strict] [--working-tree] [--json]
```

`--working-tree` reindexes from the working tree first (as `vaire index --working-tree`),
so the checks see uncommitted edits — the agent's edit→validate loop without a commit.

Checks:

- **Duplicate IDs** — two nodes sharing one `id:` (the duplicate-entity guard).
- **Dangling references** — a non-`?` reference whose target ID is not a node.
- **Frontmatter/inline drift** — a resolved reference linked **inline** whose target is
  not also in the frontmatter edge list (the actionable "declare it" direction). Advisory,
  since narrative inline links legitimately exceed the structured edge list — a *warning*,
  not a failure.
- **Orphans** — nodes with no inbound or outbound edges. A warning.

Duplicate IDs and dangling references are violations. Orphans and drift are warnings;
`--strict` promotes them to failures. Exit `0` clean, `6` on any violation (or any warning
under `--strict`).

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

`kind` is one of `duplicate_id`, `dangling_ref` (violations), `drift`, `orphan`,
`frontmatter_wikilink`, `unknown_type` (warnings). `unknown_type` flags a reference-shaped
frontmatter value (`field: team:alpha`) whose type isn't in `id_prefixes` — it was *ignored*
rather than made an edge, so the warning surfaces the silent drop (usually a type you forgot
to list). A colon in a non-reference value (a title, `summary: "TODO: …"`) is not flagged.

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
```

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

- `path` — directory to initialize (default: the current directory). Created if absent.
- Writes `<path>/.vaire/config.toml` (the committed corpus marker, with the §6 defaults)
  and a self-contained `<path>/.vaire/.gitignore` that ignores everything derived under
  `.vaire/` except `config.toml` — so `init` need not touch the repo's root `.gitignore`.
- Does **not** create a Git repo or build the index; it prints the next step (`vaire index`).
- Exit `2` if the directory is already a corpus (`.vaire/config.toml` exists) — `init`
  never clobbers an existing config.

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

Authored config that should be version-controlled lives at `.vaire/config.toml`. It is the
one committed file under `.vaire/`; everything else there is derived and gitignored via a
whitelist rule (design.md §9). All keys are optional; defaults make `vaire` work with no
config.

```toml
# .vaire/config.toml — committed, version-controlled

# Where to look (an `id:`+`type:` pair is still what makes a file a node; these only
# bound the search space).
include = ["knowledge/**/*.md", "projects/**/*.md"]
exclude = ["**/node_modules/**", "**/drafts/**", "**/archive/**"]

# Type vocabulary — the `type:` field / ID prefix in `type:id`. Growable, but
# load-bearing for *reference detection*: a frontmatter value is only treated as a
# `type:id` edge when its type is listed here, so a colon inside a non-reference value
# (a title, a note) is not mistaken for a reference. List every type you reference —
# `vaire check` warns (unknown_type) on a reference-shaped value with an unlisted type.
id_prefixes = ["person", "department", "method", "system", "event", "record", "project"]
vocabulary_strict = false

# Scoped IDs. Types listed here that carry the `scope_field` get the composed address
# `<container-id>/<type>:<local-id>` (e.g. project:atlas-2026-q2/record:2026-06-10-standup).
# Lets records use short, container-local ids that stay globally unique. Empty = off (all
# IDs flat). See §6.1.
scoped_types = []           # e.g. ["record"]
scope_field  = "scope"      # frontmatter field that supplies the scope; its value names the
                            # container (scope: project:atlas, scope: org:some-firm, …)

[embeddings]
# Pluggable, local by default:
#   "local"   — built-in, offline, no model file.
#   "command" — run via `sh -c`; receives a JSON array of strings on stdin and must return
#               a JSON array of vectors ([[f32, ...], ...]) of the same length and order.
#   "openai"  — OpenAI embeddings API; needs OPENAI_API_KEY (see Secrets, §6.2) and
#               `embedding_model`. Sends corpus text to OpenAI (data egress).
provider        = "local"                    # "local" | "command" | "openai"
command         = ""                         # used when provider = "command"
embedding_model = "text-embedding-3-small"   # used when provider = "openai"
dimensions      = 384                        # output size (v3 OpenAI models honor this)
```

Resolution order for any setting: `--config` path > `<root>/.vaire/config.toml` > built-in
defaults.

### 6.2 Secrets — `.vaire/.env`

Providers that need credentials (currently `openai`) resolve them with the precedence
**environment variable first, then `.vaire/.env`**:

- `OPENAI_API_KEY` — required for `provider = "openai"`. Set it in the shell environment,
  or in `.vaire/.env` as `OPENAI_API_KEY=sk-…`.
- `OPENAI_BASE_URL` — optional; overrides the API endpoint (proxies / Azure-style gateways).

`.vaire/.env` is `KEY=VALUE` per line (`#` comments, optional `export `, optional quotes).
It lives under `.vaire/`, which `vaire init` gitignores (only `config.toml` is committed),
so secrets are never committed. `vaire` reads it on demand — it does not mutate the process
environment.

### 6.1 Scoped IDs

By default every ID is a flat global `type:slug`, so a record needs a globally-unique
slug — projects end up hand-prefixing (`record:nova-2026-06-10-standup`), re-deciding the
prefix per project. `scoped_types` removes that tax.

When a node's `type` is in `scoped_types` **and** it carries the `scope_field` (default
`scope`), its address is a **path of typed IDs** — `<container-id>/<type>:<local-id>`:

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
- Composition is **purely local** — the scope is the node's own `scope_field` value, so no
  per-container declaration and no cross-file lookup is needed (single-pass indexing). The
  container's ID *is* the scope, so uniqueness and stability come for free (its ID is
  unique and doesn't get renamed in place).
- **The container can be any type.** The type lives in the field's value, so the same field
  scopes under different containers: `scope: project:atlas-2026-q2` →
  `project:atlas-2026-q2/record:…`, `scope: org:some-firm` → `org:some-firm/record:…`. Only
  the *field name* is configured (`scope_field`, default `scope`; e.g. set to `project` to
  tie scoping to a specific relationship field).
- The `scope:` value is also a graph edge (`ref_type: scope`) to the container — so the
  container should be a real node, or `vaire check` reports a `dangling_ref`.

**References:**

- Full anywhere: `[[project:atlas-2026-q2/record:2026-06-10-standup]]`.
- **Relative** within the same container: `[[record:2026-06-10-standup]]` (a scoped-type ref
  with no scope) resolves against the referencing node's own `scope:`. Cross-container links
  must be written in full.

**Entities stay global** — no `project:`, no scope; the entity/record split is expressed in
the ID. Nesting is one level (project) today; the grammar (a `/`-separated path of typed
segments) leaves room for deeper containers later.

### 6.3 Frontmatter references (and the `[[ ]]` trap)

Frontmatter references are **bare** — the `[[ ]]` brackets are an inline-prose convention,
not a frontmatter one. A frontmatter field value is interpreted as:

- a composed `type:id` **whose type is in `id_prefixes`** → a resolved **edge** keyed by
  the field (`org: department:platform`, `head: person:jane-doe`). A value whose prefix is
  not a configured type is left alone, so a colon in a title or note isn't mistaken for a
  reference. (`name` and `aliases` are display fields and are never scanned for references.)
- `"?type: descriptor"` / `"?: descriptor"` → an **unresolved** loose end (§6), surfaced by
  `vaire unresolved` — quote it because of the leading `?`. This lets a structured field
  name something not yet created: `head: "?person: someone senior"`. It is *not* an edge.
- anything else → ignored.

**The trap:** writing an inline-style `[[ ]]` in frontmatter (`head: [[?person: Foo]]`) is
a natural muscle-memory mistake. Unquoted, YAML parses it to nested junk (a silent no-op);
quoted, it is a meaningless string. Vairë forgivingly **strips** stray surrounding brackets
(so `head: "[[person:jane]]"` still links), but `vaire check` always **warns**
(`frontmatter_wikilink`) so the mistake surfaces rather than failing silently. The fix is
to drop the brackets: `head: person:jane` (resolved) or `head: "?person: …"` (unresolved).

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
