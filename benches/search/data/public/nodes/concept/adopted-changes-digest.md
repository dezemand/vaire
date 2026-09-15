---
id: adopted-changes-digest
type: concept
name: Adopted-changes digest
aliases: [citation digest]
---
# Adopted-changes digest

When [[cli:vaire/command:pull]] replaces a dependency's version, the question a consumer
actually has is narrower than "what changed" — it's "what changed that I rely on." The
publisher's own changelog answers the wrong question, since most of a dependency's entities
were never cited by any one consumer. The digest answers the right one by intersecting two
edge sets that already exist in the graph: the advancing
[[concept:release-record|release records']] `added`/`changed`/`retired` edges, against the
pulling package's own outbound references into that dependency.

The result is short by construction and specific to the one package that asked:

```text
1 of the 4 entities touched by 1.0.0→1.1.0 cited here:
  changed  term:torque-vectoring
```

A first pull into an empty slot reports nothing, since arriving somewhere for the first
time adopts nothing and there is no prior version to diff against. Nothing about computing
this digest can fail the pull itself — the bytes already arrived intact, so a digest that
can't be computed is a missing courtesy, never a failed fetch.
