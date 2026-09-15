---
id: store
type: component
name: Store materialization
---
# Store materialization

The pipeline `vaire pull` runs to turn a fetched [[concept:artifact]] into a usable local
package: verify its bytes against the registry's digest, unpack it with strict containment
(nothing absolute, nothing that climbs out with `..`, no symlinked entries — a refusal here
aborts the whole materialization rather than leaving a half-written directory around),
rebuild the [[concept:derived-index]] from the shipped Markdown rather than trusting the
one that shipped with it, write a `source.toml` recording exactly where these bytes came
from, seal the result read-only, and finally move it into place with one atomic rename.

The ordering matters: sealing is applied to the contents first, and only the already-sealed
directory is renamed into its final location afterward, because a rename rewrites the
*parent's* directory entry — a directory that was already read-only couldn't be moved into
place at all. See [[concept:store]] for what the result behaves like once it's there, and
[[decision:shipped-index-not-trusted]] for why the index step never just copies the
prebuilt one.
