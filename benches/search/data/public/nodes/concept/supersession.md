---
id: supersession
type: concept
name: Supersession
aliases: [tombstone]
---
# Supersession

Supersession is how a node gets retired without ever being deleted: add
`superseded_by: <type:id>` to the loser's frontmatter and strip the rest of its content.
`vaire resolve`, `backlinks`, `refs`, and search all follow the redirect transparently, so
every reference written against the old ID keeps answering — a tombstone is a promise that
an address, once published, resolves forever.

Two situations produce one: a [[finding:duplicate-id]] caught by `vaire check`, where the
better node wins and the loser points at it; and a MAJOR [[concept:release-record|release]]
that renames, merges, or relocates an [[concept:entity]], where the tombstone is what keeps
MAJOR from meaning "your links are dead" — it means "re-confirm your assumptions" instead.
A tombstone can point across a package boundary (`superseded_by: "@acme-core/team:platform"`),
and redirect-following tracks a visited set so a cycle terminates rather than looping.

Deleting a node outright, with no tombstone, is the one way to truly break a reference —
that is exactly what [[finding:dangling-ref]] exists to catch.
