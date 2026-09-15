---
id: no-capture-tier
type: decision
name: 'No third "captures" tier'
---
# No third "captures" tier

Only two systems exist: an agent's disposable memory, and the authored corpus (see
[[concept:memory-corpus-boundary]]). There is deliberately no third store sitting between
them to hold half-formed captures awaiting promotion, and no candidate table or graduation
state machine tracking what might eventually become a real record.

The reasoning is that such a tier would need its own consistency story, and there's nothing
for it to do that [[concept:commit-as-publish|committing a record]] doesn't already do more
simply: publication *is* the act of writing the record, full stop. The one place a loose
end is allowed to exist is *inline*, inside a record that's already been written — a
[[concept:loose-end]] — never in a separate staging area of its own. Rejecting a capture
tier also rules out an autonomous reference-resolution pipeline scored by confidence against
a human queue; resolution instead happens as part of ordinary authoring plus the one
deliberate, gated [[concept:entity-creation-pass]].
