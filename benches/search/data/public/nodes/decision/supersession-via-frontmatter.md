---
id: supersession-via-frontmatter
type: decision
name: Supersession via frontmatter redirect
---
# Supersession via frontmatter redirect

Merging or retiring a node is a single frontmatter field, `superseded_by: <type:id>`, and
nothing more elaborate. The [[concept:derived-index]] follows the redirect on every read
command, so references to the old address keep resolving forever — this is deliberately
the *minimum* mechanism that keeps the safety story honest, not a full merge workflow.

The harder question — how a human or agent actually decides two things are duplicates, and
what the review experience for that looks like — is explicitly left open rather than
designed prematurely; a simple redirect was judged enough to ship
[[concept:entity-creation-pass|entity creation]] safely, with richer merge tooling deferred
until real usage shows what it actually needs to do. See [[concept:supersession]] for the
full mechanics.
