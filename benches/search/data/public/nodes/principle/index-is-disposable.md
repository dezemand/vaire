---
id: index-is-disposable
type: principle
name: Index is disposable
---
# Index is disposable

Nothing about the [[concept:derived-index]] is precious. It rebuilds from the committed
files in seconds, so a corrupt database, a mismatched [[?concept: schema version marker]],
or simple curiosity about a clean rebuild are all handled the same way: delete it, run
[[cli:vaire/command:index]] `--full`, move on. A schema change bumps a version number and
the next plain index build migrates by rebuilding from scratch rather than by patching
rows in place.

Treating the index as disposable is what makes several other decisions cheap. Adopting a
pre-1.0 storage engine ([[decision:turso-engine]]) carries little risk, since any
regression it introduces is one rebuild away from disappearing. A pulled release's shipped
index is discarded outright and rebuilt locally rather than trusted (see
[[decision:shipped-index-not-trusted]]), because a claim about the graph is worth nothing
next to a file every consumer can regenerate for themselves.
