---
id: refs
type: command
scope: cli:vaire
name: vaire refs
---
# vaire refs

`vaire refs <id> [--depth N] [--type T]` lists the nodes `<id>` itself references —
outbound edges, the mirror of [[cli:vaire/command:backlinks]]. Depth beyond the default of
1 traverses further and returns a flattened, de-duplicated set with each node's shortest
distance from the start, sorted by `(distance, id)`.

Unresolved [[concept:loose-end|loose ends]] never appear here, by definition — they aren't
edges, so they don't traverse; [[cli:vaire/command:unresolved]] is the separate command for
those. Crossing a package boundary follows the edge through its *owning* package's own
dependency links rather than the caller's, and a dangling cross-package target is simply
dropped from the walk the same way a local one would be, surfaced instead by
[[cli:vaire/command:check]].
