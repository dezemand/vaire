---
id: loose-end
type: concept
name: Loose end
aliases: [unresolved reference, descriptor reference]
---
# Loose end

A loose end is what you write when you want to reference something that has no ID yet:
`[[?person: someone from logistics]]` inline, or `head: "?person: someone senior"` in
frontmatter. It carries a *descriptor* — what the author actually saw — never a guessed
slug. Two authors guessing slugs for the same referent is the duplicate-[[concept:entity]]
problem wearing a question mark; a descriptor doesn't collide with anything.

Loose ends are not edges — they never resolve, traverse, or count toward a
[[finding:dangling-ref]] check. They surface fresh on every call to
[[cli:vaire/command:unresolved]], which is the work list for the
[[concept:entity-creation-pass]]. Resolving one is the one sanctioned in-place edit of a
record: an ID gets added and the original phrasing survives as display text, so the record
stays additive (see [[principle:additive-authoring]]).

## Why not just guess a name

A provisional slug is still a slug. Recording one turns an honest "I don't know what this
is" into a false claim of identity, and the corpus has no way to tell a real entity from a
placeholder later. Carrying only a description keeps the door open for the gated pass to
decide, once, what this thing actually is.
