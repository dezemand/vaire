---
id: unresolved
type: command
scope: cli:vaire
name: vaire unresolved
---
# vaire unresolved

`vaire unresolved [--type T] [--scope container-id] [--all-packages]` lists every
[[concept:loose-end]] currently sitting in the corpus — the live work list for the
[[concept:entity-creation-pass]], derived fresh from the files on each call with nothing
stored in between. `--type` filters by the `?type` hint a descriptor carries (a bare
`[[?: …]]` has no type and only matches when the flag is omitted entirely).

Its default scope is deliberately narrow — this package only, never its dependencies —
because a descriptor is package-agnostic and a dependency's own open questions belong to
*its* maintainer's worklist, not to every consumer's. `--all-packages` widens to the linked
closure when that's genuinely wanted; it can't be combined with `--scope`. Run from a
[[concept:rootless-session]] with no package at all, it behaves as `--all-packages` over
the whole catalog by default.
