---
id: workspace-resolver
type: component
name: Workspace resolver
---
# Workspace resolver

Resolves what a declared [[concept:dependency]] actually points at on this machine:
checking `.vaire/packages/<name>` first, falling back to the run-root package itself when
the name matches it, then to the run-root's own links, and — since v0.3 — consulting the
[[concept:catalog]] to satisfy anything still unresolved by finding a live working copy
whose manifest declares the right name and whose version fits the `^MAJOR` constraint.

Only commands that already write links ever invoke this resolver's write path — `vaire add`
and the ensure pass inside [[cli:vaire/command:index]] and [[cli:vaire/command:check]] —
while every read command only ever consults links that already exist, never materializing
new ones. Ambiguity (two live candidates satisfying one constraint) and conflict (two
closure members constraining one name to disjoint majors) are both surfaced by name rather
than resolved by any kind of tiebreak, consistent with [[?principle: reported never
guessed]].
