---
id: orphan
type: finding
name: Orphan
aliases: [orphan node]
---
# Orphan

A [[concept:node]] with no inbound edges and no outbound edges at all — nothing points at
it, and it points at nothing. Reported as a warning, since an orphan isn't broken in the
way a [[finding:dangling-ref]] is; it simply isn't woven into the graph yet.

Most orphans are just under-linked and the fix is to connect them — link the node from
somewhere relevant, or add outbound references to the things its own prose already talks
about. But an orphan that would never plausibly be referenced from anywhere is a different
signal entirely: it may be failing the substance-floor admission test for a standalone
[[concept:entity]], in which case the right fix is folding its content into its parent as a
section rather than linking it defensively just to silence the warning.
