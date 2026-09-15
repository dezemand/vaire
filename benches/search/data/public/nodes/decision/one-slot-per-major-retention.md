---
id: one-slot-per-major-retention
type: decision
name: One slot per major line
---
# One slot per major line

The [[concept:store]] keeps at most one version per major line for any given dependency,
plus anything explicitly pinned. Pulling 1.4.2 quietly replaces 1.4.1 in place; two majors
coexist only when the dependency closure genuinely constrains both at once. This is safe
specifically *because* [[decision:caret-major-only|the `^MAJOR` constraint]] guarantees
within-major substitutability as a protocol-level promise, so the retention rule is derived
from that guarantee rather than being a separately-justified space optimization.

A pin survives replacement, since pinning exists precisely to opt out of that
substitutability promise for one dependency. Removal is never treated as fatal even when it
fails outright — the pull it would have followed already succeeded, so a stray sibling
version left on disk costs nothing but a little space, never correctness. See
[[?command: the clean command]] for how leftover versions that satisfy none of these
conditions eventually get swept away.
