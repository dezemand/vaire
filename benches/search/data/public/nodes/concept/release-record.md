---
id: release-record
type: concept
name: Release record
aliases: [release entity]
---
# Release record

Every [[cli:vaire/command:release]] writes an entity describing itself —
`releases/<version>.md` by convention — carrying the date, the computed bump, and edges to
every [[concept:entity]] it `added`, `changed`, or `retired`. Making the changelog *corpus*
rather than a separate file is what turns "which releases touched this entity?" into an
ordinary [[cli:vaire/command:backlinks]] query instead of a search through prose, and it's
the other half of what makes the [[concept:adopted-changes-digest]] possible.

`removed` entities are recorded as plain text, not references — a removed entity has no
address left to point at, and that's fine because removals only ever happen inside a
gated MAJOR anyway. A release record's edges are also treated as history: they may
legitimately cite an entity a *later* release goes on to remove, and that citation is
exempt from ordinary dangling-reference checking, because it's a true statement about what
was published at the time, not a mistake to fix.
