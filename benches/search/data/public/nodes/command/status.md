---
id: status
type: command
scope: cli:vaire
name: vaire status
---
# vaire status

`vaire status` reports the health of the [[concept:derived-index]] without changing
anything: which commit it was last built from and how far behind HEAD that is, node and
edge counts by type, embedding cache coverage, and one row per
[[concept:linked-package|linked]] dependency showing its own freshness and embedding
provider. It's the one read-adjacent command that tolerates a completely missing index,
reporting "not built yet" and exiting `0` rather than failing outright the way an ordinary
read command would.

It also ambiently reports the pending [[cli:vaire/command:release]] — "would be minor, 3
new, 12 changed" — so a maintainer is never surprised by what the next release turns out to
be, as long as the index itself is current with HEAD.
