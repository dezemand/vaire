---
id: duplicate-id
type: finding
name: Duplicate ID
aliases: [duplicate_id]
---
# Duplicate ID

Raised when two nodes share one composed address — the same `type:id` pair claimed by two
files. This is a violation, not a warning: [[cli:vaire/command:check]] fails outright,
because ID uniqueness is the one guarantee every other integrity check and every
[[concept:reference]] in the corpus depends on silently holding.

The sanctioned fix is never to delete either file. Decide which node is the real one, then
add `superseded_by: <the-winner>` to the loser and strip its content down to that redirect
— see [[concept:supersession]]. If the two files turn out to describe genuinely different
subjects that only happened to collide on a slug, the one that was never published gets a
new slug instead; a slug that already shipped in a release is never recycled, published or
not.
