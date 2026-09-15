---
id: deps
type: command
scope: cli:vaire
name: vaire deps
---
# vaire deps

`vaire deps` prints the resolved [[concept:dependency]] tree exactly as
[[component:workspace-resolver|live link inspection]] finds it — no
[[concept:derived-index]] required, which makes it a safe first command to run in a
workspace that hasn't been indexed at all yet. Each member's own dependencies resolve
through *its* manifest and *its* links, the identical keying every cross-package reference
uses.

A dependency cycle prints once, annotated `(cycle)`, and is never descended into further;
an unresolvable name shows `MISSING` with the exact fix; a linked version whose major falls
outside the declared `^N` is flagged (surfaced only — see
[[finding:dependency-version-mismatch]]). It always exits `0`: reporting the tree is its
whole job, and judging whether that tree is *healthy* belongs to
[[cli:vaire/command:check]] instead.
