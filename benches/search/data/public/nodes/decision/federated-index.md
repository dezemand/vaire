---
id: federated-index
type: decision
name: The index stays federated
---
# The index stays federated

There is no merged, workspace-wide database. Every [[concept:package]] — the current one,
each [[concept:linked-package|linked]] sibling, and eventually each store entry — carries
exactly its own `.vaire/index.db`, built from its own manifest, bound to its own commit,
holding its own embeddings. A cross-package read opens the dependency's index in place and
composes results in the CLI layer, rather than any single database ever holding rows from
two packages at once.

The payoff shows up in several places at once: IDs and paths can never collide across
package boundaries, because nothing ever merges them into shared storage; a dependency's
vectors are computed exactly once and reused by every consumer rather than recomputed per
consumer; and a [[concept:registry]] can ship a package together with its own pre-built,
pre-embedded index, so adopting it costs a consumer nothing extra — right up until
[[decision:shipped-index-not-trusted|materialization discards that shipped index anyway]]
and rebuilds locally, which is a separate decision layered on top of this one.
