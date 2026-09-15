---
id: additive-authoring
type: principle
name: Additive authoring
---
# Additive authoring

A corpus only ever grows forward. A [[concept:record]] is immutable once written — to
change what it says, author a new record, never edit the old one's prose. The single
sanctioned in-place edit anywhere in the system is resolving a
[[concept:loose-end]], and even that is additive by construction: it adds an ID target and
keeps the author's original phrasing intact as display text, so nothing about what was
actually said ever changes, only what it now also points at.

This is what keeps history honest without needing an append-only database to enforce it —
Git already gives that for free. It's also the property that makes free-form autonomous
authoring survivable: since nothing can be silently rewritten, the worst an author can do is
add something wrong, which a maintainer corrects by adding a [[concept:supersession]]
tombstone or a corrective record, never by reaching back to erase the mistake.
