---
id: documents-as-evidence
type: principle
name: Documents as evidence
---
# Documents as evidence

A corpus document doesn't merely assert a fact — the way it was written and committed *is*
evidence for that fact. The Git commit that introduced a [[concept:record]] names who wrote
it, when, and — through the commit message — why, natively rather than through any
bookkeeping the tool has to build.

The same posture shapes how [[concept:release-record|release records]] behave once
published: their `added`/`changed`/`retired` edges are treated as a historical statement
about what a given version actually contained, not a live claim that has to keep matching
the present tree. A release record is allowed to cite an entity a *later* release goes on to
remove, and that citation is deliberately exempt from dangling-reference checking — it
would be a lie to "fix" it, since it accurately describes what was true when it was
written. Evidence doesn't get corrected after the fact; superseding or retiring is how the
corpus moves forward instead (see [[principle:additive-authoring]]).
