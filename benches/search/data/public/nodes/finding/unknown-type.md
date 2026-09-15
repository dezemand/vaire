---
id: unknown-type
type: finding
name: Unknown type
aliases: [unknown_type]
---
# Unknown type

A frontmatter value matches the [[concept:reference]] target grammar by shape —
`field: team:alpha` — but `team` isn't declared in this package's
[[concept:type-vocabulary]]. Identification (does this look like a reference at all) and
classification (is its type actually declared) are two separate steps by design, and this
finding is exactly the residual ambiguity classification produces: the value was *not*
silently turned into an edge, and it was *not* silently treated as a harmless string
either. It's surfaced instead.

Two legitimate fixes exist, and choosing between them is a judgment call, never automatic:
if it was truly meant as a reference, declare the type deliberately (see the type-growth
ladder referenced from [[concept:type-vocabulary]]) — never as a reflex just to silence the
warning. If it only happens to contain a colon and was never meant as a reference at all,
quote it as a plain string instead.
