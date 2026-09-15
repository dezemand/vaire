---
id: package
type: concept
name: Package
---
# Package

A package is a directory tree with a committed [[concept:manifest]] (`knowledge.toml`) at
its root — the unit of ownership and consumption for a body of knowledge. Its identity is
**declared**, never derived from the directory it lives in: the manifest's `name` is what
other packages depend on, so a package can be relocated, vendored, or fetched into a cache
without a single reference anywhere needing to change.

Packages exist for two reasons. **Autonomy** — a body of knowledge that moves and versions
on its own cadence deserves its own boundary rather than living inside someone else's.
**Selective consumption** — a consumer who wants *this* knowledge without everything
alongside it can declare exactly that [[concept:dependency|dependency]] and nothing more.
Neither reason is "it got big" or "it felt tidy": granularity follows ownership, and a
split that never versions independently of its one consumer was premature and should be
merged back.

Two packages may freely declare the same [[concept:type-vocabulary|type]] name or the same
local `id:` — nothing collides, because a [[concept:linked-package|cross-package]]
reference is always explicitly qualified with `@name/`, and the [[concept:derived-index]]
stays **federated**: every package, current or linked, keeps its own database, built from
its own manifest and its own commit.
