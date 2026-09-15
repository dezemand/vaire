---
id: alias
type: concept
name: Alias
---
# Alias

An `aliases:` list on a node is a display field, never a reference — but it is load-bearing
for lookup. `vaire suggest` and the matching step of the [[concept:entity-creation-pass]]
check a descriptor against `name`/`aliases` before they touch full text or embeddings,
because a short phrase like "the ingest service" hits an alias far more reliably than a
vector ever will.

A rich alias list — abbreviations, old names, nicknames, translations — is what keeps the
next loose end from becoming an accidental duplicate [[concept:entity]]. Authors are
expected to harvest aliases from source material up front, and a [[?principle: contributor
role guideline]] permits adding an alias to an existing node at any time without going
through the gated pass, because doing so adds no new identity.
