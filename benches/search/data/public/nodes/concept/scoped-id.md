---
id: scoped-id
type: concept
name: Scoped ID
aliases: [scoped identifier, container-scoped address]
---
# Scoped ID

A scoped ID is a path of typed IDs rather than a single `type:id` pair:
`<container-id>/<type>:<local-id>`, e.g. `cli:vaire/command:search`. Scoping is
**data-driven** — any [[concept:node]] carrying the configured `scope_field` (default
`scope`) gets this composed address, regardless of its type, so a container can hold any
mix of scoped children without a separate declaration anywhere else.

The node itself only ever writes a short local `id:`; uniqueness is required within the
container, not globally, which removes the hand-prefixing tax a flat namespace forces on
every child (`command:vaire-search`, `command:vaire-index`, …). A bare `[[type:id]]`
written *inside* a scoped node resolves scope-first, then falls back to the global
namespace if no sibling exists — so a record can reference its container's other children
without spelling out the full path.

`scoped_types_whitelist`/`scoped_types_blacklist` in `knowledge.toml` are a lint only —
`vaire check` warns with [[finding:scoped-type-not-permitted]] when a scoped node's type
isn't on the allowed list, but the node still indexes either way.
