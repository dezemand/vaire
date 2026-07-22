---
name: vaire-query-cli
description: How to query a Vairë reference-graph index from a shell with the `vaire` CLI. Covers the read commands resolve, render, backlinks, refs, search, suggest, unresolved, and deps (including cross-package `@pkg/type:id` ids and linked dependencies), plus maintenance (init, index, check, status), their flags, JSON output, and exit codes. Use this when looking up an entity or record, following references or backlinks, traversing the graph, searching the corpus, listing unresolved references, rendering a node to portable Markdown, resolving scoped record IDs (`container-id/type:local`) or scope-filtering, or building/validating the index of a Vairë knowledge base from the command line.
metadata:
  project: vaire
---

# Querying Vairë from the CLI

`vaire` is a single binary over a derived index of a Markdown knowledge package. For how the
corpus files themselves work, see the **vaire-files** skill.

## Before you query

- The package must have a committed `knowledge.toml` manifest. Create one with `vaire init`
  (run it in the package root, or `vaire init <path>`).
- The index must be **built**: run `vaire index`. A read command before the index exists
  exits `4` and tells you to run `vaire index`. `vaire` never builds as a side effect.
- The root is found by walking up to the nearest `knowledge.toml`; override with
  `--repo <path>`.
- Re-run `vaire index` after editing/committing files, or queries see stale data. (In a
  Git repo it indexes the committed tree — commit first; otherwise it reads the working
  tree.)
- **Editing in a loop (agents):** to validate uncommitted edits without committing, use
  `vaire index --working-tree` (then query/`check`), or `vaire check --working-tree` which
  reindexes the working tree first. The index then reflects your edits until the next plain
  `vaire index`. This turns the loop into edit-many → validate → commit, instead of
  commit-then-learn.

## Read commands

All accept `--json`. They return **pointers** (node IDs + repo-relative paths, and for
search, section anchors) — open the file for depth — except `render`, which returns the
node body.

| Command | Purpose |
| --- | --- |
| `vaire resolve <id>` | Locate a node: path, type, frontmatter. Follows `superseded_by`. Exit `5` if `<id>` is not a node. |
| `vaire render <id>` | The node as portable Markdown: frontmatter kept, `[[type:id]]` resolved to `[name](relative-path)`. |
| `vaire backlinks <id> [--type T] [--limit N]` | Nodes that reference `<id>` (inbound edges). |
| `vaire refs <id> [--depth N] [--type T]` | Nodes `<id>` references (outbound edges). `--depth >1` traverses; unresolved `[[?...]]` never appear. |
| `vaire search <query> [--type T] [--scope project:id] [--limit N] [--local]` | Hybrid full-text + vector search over this package **and its linked dependencies**; returns files with matching section anchors. `--limit` default 10; `--local` = this package only. |
| `vaire suggest <descriptor> [--type T] [--limit N] [--local]` | Lookup-before-reference: ranked existing IDs a descriptor might be (name/aliases first, prose backup). Dependency hits arrive pre-qualified (`@pkg/type:id`), ready to paste. |
| `vaire unresolved [--type T] [--scope project:id] [--all-packages]` | Every `[[?...]]` loose end — this package's worklist by default; `--all-packages` widens to linked dependencies. |
| `vaire deps` | The resolved linked-dependency tree (live link inspection; no index needed; always exit 0). |

`<id>` is the node's address. Global nodes are `type:id` (`person:jane-doe`,
`department:hr`). **Scoped** nodes (any node carrying a `scope:`) are a path —
`<container-id>/type:id`, e.g.
`project:atlas-2026-q2/record:2026-06-10-standup`. Pass the full address to
`resolve`/`render`/`backlinks`/`refs`; don't hand-build it — get it from `search`/`resolve`
or from `unresolved`. `--scope <container-id>` (e.g. `--scope project:atlas-2026-q2`)
filters `search`/`unresolved` to nodes scoped under that container. Under `--scope`, search
result IDs are shown **scope-relative** (the prefix is omitted — `record:sync`, not
`project:atlas-2026-q2/record:sync`); without it, scoped nodes show the full path. See the
**vaire-files** skill for how scoped IDs are composed and referenced (relative vs full).

**Cross-package** ids carry an `@<package>/` qualifier (`@acme-core/team:platform`) and
work in every read command — the target lives in a linked dependency
(`.vaire/packages/<name>`). Cross-package results carry a `package` field in `--json`
(with `path` staying package-root-relative) and consumer-relative paths in human output.
Reads that fan out list unavailable dependencies under `skipped` rather than silently
dropping them; run `vaire index` to (re)build linked dependency indexes.

## Maintenance commands (not for agents over MCP)

| Command | Purpose |
| --- | --- |
| `vaire init [path]` | Scaffold `knowledge.toml` so the dir is a discoverable corpus (migrates a legacy `.vaire/config.toml`). Exit `2` if already one. |
| `vaire index [--full] [--working-tree] [--re-embed] [--no-deps]` | Build/rebuild the index — and refresh each linked dependency's own index (`--no-deps` skips). `--full` is a cold rebuild; `--working-tree` indexes uncommitted edits (records no commit); `--re-embed` re-embeds every section with the current provider, bypassing the cache. Run plain from a `post-commit` hook. |
| `vaire check [--strict] [--working-tree] [--no-deps]` | Integrity: duplicate IDs, dangling references (local **and** cross-package), undeclared imports, and missing dependencies are failures (exit `6`); orphans, drift, frontmatter-`[[ ]]`, `unknown_type`, unused dependencies, and version mismatches are warnings (`--strict` promotes them). Runs the dependency ensure pass first (`--no-deps` skips); `--working-tree` reindexes the working tree first. |
| `vaire status` | Index state: last-indexed commit, commits behind HEAD, node/edge/embedding counts, and one row per linked dependency (freshness, lag, embedding provider). Tolerates a missing index. |
| `vaire add <pkg>[@^N] [--link <path>]` | Declare a dependency in `knowledge.toml` and (with `--link`) wire `.vaire/packages/<pkg>` to where it lives. |

## Global flags

`--repo <path>` (corpus root, overrides discovery and `VAIRE_REPO`), `--json`,
`--config <path>`, `--quiet`/`-q`, `--no-color`.

## Typical flows

```bash
# Lookup-before-reference (authoring, per the vaire-files contract)
vaire suggest "the logistics team" --type department   # → ranked candidate IDs
vaire resolve department:logistics                      # confirm the chosen one

# Explore the graph
vaire backlinks person:jane-doe --type record
vaire refs record:2026-06-10-broker-sync --depth 2

# The unresolved work list, scriptably
vaire unresolved --type person --json | jq '.unresolved[].descriptor'

# Get clean, link-resolved Markdown for a node
vaire render department:hr
```

## JSON output

With `--json`, stdout is **always** a single JSON value — including errors, emitted as
`{"error": {"code": …, "kind": …, "message": …}}` — so a consumer parses one shape.
Results are sorted deterministically. Example shapes:

- `resolve` → `{ "id", "type", "path", "frontmatter": {…}, "superseded_by": null }`
- `backlinks` → `{ "id", "backlinks": [{ "id","type","path","ref_type","line" }], "count" }`
- `refs` → `{ "id", "depth", "refs": [{ …, "distance" }], "count" }`
- `search` → `{ "query", "results": [{ "id","type","path","score","anchors":[{"heading","line","snippet"}] }], "count" }`
- `unresolved` → `{ "unresolved": [{ "record","path","type_guess","descriptor","line" }], "count" }`

## Exit codes

`0` success / no violations · `2` usage error · `3` index corrupt (rebuild with
`vaire index --full`) · `4` no corpus found, or index not built · `5` ID not found · `6`
`vaire check` found violations (or warnings under `--strict`).
