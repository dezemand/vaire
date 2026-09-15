---
id: dangling-ref
type: finding
name: Dangling reference
aliases: [dangling_ref]
---
# Dangling reference

A resolved (non-`?`) [[concept:reference]] whose target doesn't exist — locally, or, since
the cross-package resolution lints landed, an `@pkg/type:id` whose target still isn't found
after following every [[concept:supersession|tombstone]] in the owning package's own
context. Existence is all that's checked, so the dependency's own declared
[[concept:type-vocabulary]] is irrelevant to whether this fires.

Three distinct fixes apply depending on what actually happened: a typo gets corrected
(`vaire suggest` finds the intended target fast); a target that genuinely doesn't exist
gets demoted to a [[concept:loose-end]] rather than having an entity minted just to fill the
hole, since minting one is the gated [[concept:entity-creation-pass]]'s job, never a
side-effect of fixing a check failure; and a target that used to exist but was deleted
outright gets restored as a proper tombstone instead. This is always a violation — never a
warning — because a shipped reference to nothing is broken for every consumer, not just the
author who wrote it.
