---
id: working-copy-outranks-release
type: decision
name: A working copy outranks a pulled release
---
# A working copy outranks a pulled release

Resolution order is fixed and deliberate: an explicit link, then the run-root package
itself, then the [[concept:catalog|catalog]]'s live working copies, and only last the
[[concept:store]]. A checkout of the same package always wins over a pulled release of it,
even an up-to-date one — because a checkout is what's actually being authored right now,
and silently answering from a published copy instead would mean quietly reading yesterday's
version while believing it's current.

The corollary is what makes reproducibility meaningful at all: only an answer resolved
from the store can ever be reproduced elsewhere, since only a store entry carries a
checksum in the [[concept:lockfile]]. `vaire --frozen` is the flag that turns this into
enforcement rather than convention — it answers strictly from the store and refuses any
dependency that would otherwise resolve to a working copy, naming the pull that would fix
it, which is the posture CI and autonomous agents want and ordinary authoring explicitly
does not.
