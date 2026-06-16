---
name: vaire-query-mcp
description: How to query a Vairë reference-graph index through its STDIO MCP server, started with `vaire mcp`. The server exposes the read tools resolve, render, backlinks, refs, search, and unresolved; their results are the CLI `--json` shapes verbatim. Use this when an agent should query a Vairë knowledge corpus via MCP tools rather than shelling out — registering/starting the server, calling the tools with the right arguments (including scoped record IDs like `container-id/type:local`), and interpreting their JSON results and errors.
metadata:
  project: vaire
---

# Querying Vairë over MCP

`vaire mcp` starts a Model Context Protocol server over STDIO (JSON-RPC 2.0). It exposes
the corpus's **read** commands as MCP tools — the tools *are* the CLI read commands, and
each tool result is that command's `--json` shape, verbatim. For the corpus file model see
the **vaire-files** skill; for the same operations from a shell see **vaire-query-cli**.

## Prerequisites

- The corpus must have a built index (`vaire index`). The MCP server operates against the
  already-built index and **never builds or writes** it — if the index is missing, tool
  calls return an error pointing at `vaire index` (it will not build on your behalf).
- One server instance serves one repo, resolved at startup by walking up to a `.vaire/`
  dir, or via `--repo <path>`.

## Registering / starting the server

Run `vaire mcp` (optionally `vaire mcp --repo /path/to/corpus`) as a STDIO MCP server.

Generic MCP client config:

```json
{
  "command": "vaire",
  "args": ["mcp", "--repo", "/path/to/corpus"]
}
```

Claude Code:

```bash
claude mcp add vaire -- vaire mcp --repo /path/to/corpus
```

## Tools

Maintenance commands (`init`, `index`, `check`, `status`) are **not** exposed — the
agent-facing surface is bounded to reads. The six tools and their arguments:

| Tool | Arguments | Returns |
| --- | --- | --- |
| `resolve` | `id` (required) | Node location + frontmatter; follows `superseded_by`. |
| `render` | `id` (required) | The node as portable Markdown (frontmatter kept, links resolved). |
| `backlinks` | `id` (required), `type`, `limit` | Nodes referencing `id` (inbound edges). |
| `refs` | `id` (required), `depth`, `type` | Nodes `id` references (outbound edges); `depth>1` traverses. |
| `search` | `query` (required), `type`, `scope`, `limit` | Hybrid full-text + vector search; files with section anchors. |
| `suggest` | `descriptor` (required), `type`, `limit` | Ranked existing IDs a descriptor might be (name/aliases first). Lookup-before-reference. |
| `unresolved` | `type`, `scope` | Every `[[?...]]` loose end in the corpus. |

`id` is the node's address: global nodes are `type:id` (`person:jane-doe`), while
**scoped** nodes (corpora with `scoped_types`, commonly `record`) are a path —
`<container-id>/type:id`, e.g. `project:atlas-2026-q2/record:2026-06-10-standup`. Pass the
full address to `resolve`/`render`/`backlinks`/`refs` (obtain it from `search`/`resolve`,
don't construct it). `scope` is a container ID (`project:atlas-2026-q2`, or any container
type like `org:some-firm`) that filters `search`/`unresolved`. The **vaire-files** skill
covers how scoped IDs compose and how to reference them.

## Interpreting results

A tool result's text content is the command's JSON output (same shapes as in the
**vaire-query-cli** skill). For example:

- `resolve` → `{ "id", "type", "path", "frontmatter": {…}, "superseded_by": null }`
- `search` → `{ "query", "results": [{ "id","type","path","score","anchors":[…] }], "count" }`

Results return **pointers** (IDs + repo-relative paths; search adds section anchors) — open
the file, or call `render`, for the full body.

**Errors.** A command-level failure (ID not found, missing index) comes back as a tool
result with `isError: true`, whose text is `{"error": {"code", "kind", "message"}}` — the
same shape the CLI emits (e.g. `code: 5` for an unknown ID). A malformed call (unknown
tool, missing required argument) is a JSON-RPC protocol error.

## Typical agent flow

1. **Lookup before referencing** — `suggest` a descriptor to get ranked candidate IDs, then
   `resolve` the chosen one (the authoring contract in the vaire-files skill).
2. **Traverse** — `backlinks` / `refs` to walk the graph.
3. **Read for depth** — open the returned path, or `render` the node for clean Markdown.
4. **Find loose ends** — `unresolved` for the `[[?...]]` work list.
