---
id: node
type: concept
name: Node
---
# Node

The unit Vairë actually operates on. Any Markdown file whose frontmatter carries both an
`id:` (a bare, locally-unique slug) and a `type:` is a node; its address is the composition
of the two, `type:id`. Everything else in a repository — a README with no frontmatter, a
draft, an image — is prose or payload the index simply ignores.

Vairë is deliberately agnostic to *why* a file is a node: [[concept:entity]] and
[[concept:record]] are corpus conventions layered on top by authors, not a distinction the
engine enforces. This is also why discovery is by frontmatter and not by directory —
`id:`/`type:` decide what a thing is, so a node can move to any path without breaking a
single reference to it, and a [[concept:scoped-id|scoped]] node's container comes from its
own `scope:` value rather than from where the file happens to sit.
