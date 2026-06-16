---
id: ingest-api
type: system
name: Ingest API
aliases: [ingest, ingest-api]
owner: department:platform
implements: method:event-sourcing
status: active
updated: 2026-06-15
---
# Ingest API

Stream ingestion service owned by [[department:platform]]. Built on
[[method:event-sourcing]].

Note how `owner:` and `implements:` in the frontmatter are ordinary edge fields — any
frontmatter value that parses as a `type:id` becomes an edge keyed by its field name.
