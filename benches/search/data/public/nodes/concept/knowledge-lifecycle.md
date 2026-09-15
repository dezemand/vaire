---
id: knowledge-lifecycle
type: concept
name: Knowledge lifecycle
aliases: [corpus lifecycle]
---
# Knowledge lifecycle

The arc a piece of knowledge follows from first mention to something a stranger can
depend on. It starts as a [[concept:loose-end]] inside a freshly authored
[[concept:record]] — a descriptor, not yet an identity. Committing that record is already
[[concept:commit-as-publish|publication]] within the package: the fact is real and
findable the moment `vaire index` picks it up.

Identity only arrives later, deliberately, through the [[concept:entity-creation-pass]],
which either links the descriptor to something that already exists or mints a new
[[concept:entity]] from the cluster of descriptions that pointed at it. From there the
entity accretes references the way any node does, until a maintainer decides accumulation
warrants a statement: [[cli:vaire/command:release]] computes a version, writes a
[[concept:release-record]], and — once pushed to a [[concept:registry]] — the knowledge
becomes something [[cli:vaire/command:pull]] can fetch into a stranger's
[[concept:store]], sealed and citable, with the [[concept:adopted-changes-digest]] telling
them exactly what changed that they actually rely on.

Every step in this arc is additive; nothing along the way ever rewrites what came before
it, which is the property that makes the whole chain trustworthy end to end.
