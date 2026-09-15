---
id: linked-package
type: concept
name: Linked package
---
# Linked package

A linked package is how a declared [[concept:dependency]] becomes usable: an entry at
`.vaire/packages/<name>`, a symlink (or real directory) whose target is a package whose own
manifest declares that exact `name` back — a mismatch is refused outright rather than
guessed at, because identity here is always declared, never path-derived. `vaire add <name>
--link <path>` writes both the manifest entry and this link in one step.

The split matters: the committed manifest carries only the version contract, so it stays
machine-independent, while the link itself is per-checkout state sitting under the
already-gitignored `.vaire/`. Resolving a cross-package `@pkg/type:id` reference walks a
short, specific order — the referencing package's own link, then the run-root package
itself if the name matches it, then the run-root's own links as a fallback — which is what
lets one flat set of links at the top of a workspace serve an entire transitive closure
without every intermediate package wiring its own copy.
