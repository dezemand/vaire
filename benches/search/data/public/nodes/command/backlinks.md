---
id: backlinks
type: command
scope: cli:vaire
name: vaire backlinks
---
# vaire backlinks

`vaire backlinks <id> [--type T] [--limit N]` lists every node with an inbound
[[concept:reference]] to `<id>` — the graph run in reverse from
[[cli:vaire/command:refs]]. Sorted by the referencing node's own ID ascending, with
`ref_type` on each row naming whether the edge came from a frontmatter field, an inline
wikilink, or a diagram marker.

It's the query behind "which releases touched this entity" (filter to `--type release`)
and behind auditing whether anything still depends on a node before quietly folding it into
its parent. A cross-package call gathers referencing nodes from the entire dependency
closure, each member consulted through its own aliases for the target package — visibility
here is scoped to "what you depend on," never a workspace-wide reverse index.
