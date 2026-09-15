---
id: store
type: concept
name: Store
---
# Store

The store, `~/.vaire/store/<name>/<version>/`, is where `vaire pull` lands a fetched
[[concept:artifact]]: verified, unpacked, re-indexed with the consumer's own tools, and
sealed read-only. A store entry behaves exactly like any other package directory to
everything above it — the resolver links to one the same way it links to a checkout, and no
read command knows or cares which kind it got.

Retention keeps **one slot per major line, plus anything pinned**: pulling 1.4.2 quietly
replaces 1.4.1 because within-major substitutability is the protocol's own promise, while
two majors coexist whenever the dependency closure genuinely needs both. Sealing applies to
the corpus files and the provenance record (`source.toml`), but never to the index
directory itself — a database has to be opened read-write even to read it, so a sealed
index would simply be unreadable. Immutability here is enforced by policy (nothing ever
rebuilds an entry in place), with the filesystem permissions only as a backstop.
