---
id: typed-ids
type: decision
name: Typed IDs
---
# Typed IDs

Every address is `type:id`, never a bare slug. The `type:` half is both the authoritative
type of the node and its ID namespace, which means the index always knows what kind of
thing an address refers to without ever having to read a byte of prose to find out — and it
means two entities of different types can share the same local slug with zero risk of
collision.

This is what makes the [[concept:type-vocabulary]] load-bearing rather than cosmetic:
declaring a new type is effectively opening a new namespace, which is part of why growing
it is treated as deliberate, reluctant maintainer work rather than something that happens
by accident the first time someone writes a colon in frontmatter that wasn't meant as a
reference at all.
