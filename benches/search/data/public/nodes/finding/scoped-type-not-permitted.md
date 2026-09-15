---
id: scoped-type-not-permitted
type: finding
name: Scoped type not permitted
aliases: [scoped_type_not_permitted]
---
# Scoped type not permitted

A [[concept:scoped-id|scoped]] node's own type doesn't satisfy the manifest's
`scoped_types_whitelist`/`scoped_types_blacklist` lint policy. This is purely advisory —
scoping itself stays data-driven off the presence of a `scope:` field regardless of type,
so the node composes and resolves its address exactly the same whether or not this warning
fires; nothing here is an actual gate on behavior.

Two equally valid fixes exist, and which one applies is a judgment call: either the scoping
on this particular node is a mistake and should be dropped, or the policy itself is simply
out of date and a maintainer should widen `scoped_types_whitelist` (or narrow the
blacklist) to reflect how the package is actually being used now.
