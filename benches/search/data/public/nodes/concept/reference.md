---
id: reference
type: concept
name: Reference
---
# Reference

A reference points from one [[concept:node]] at another, by ID — never by display name or
file path. It shows up in two syntactic places: inline as `[[type:id]]` (optionally
`[[type:id|Display text]]`), or as a bare frontmatter value matching the same target
grammar (`owner: department:hr`), which becomes a [[concept:frontmatter-edge]] keyed by its
field name.

A reference is always in exactly one of two states: **resolved**, where the target type
and id are real and it becomes a graph edge; or a [[concept:loose-end]], marked by a
leading `?`, which carries a descriptor instead of an ID and is never an edge. Identifying
whether something even *is* a reference is purely a matter of shape — a narrow charset that
structurally excludes URLs, emails, times, and dates — decoupled from whether its type
happens to be declared, which is a separate, later question ([[finding:unknown-type]]).
