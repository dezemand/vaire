# Example corpus

A small, self-consistent knowledge corpus demonstrating the Vairë model
([spec/design.md](../spec/design.md), [spec/cli.md](../spec/cli.md)). This file has no
`id:`/`type:`, so it is **not** a node — it is just documentation.

## What's here

```
.vaire/config.toml                     committed config (type vocabulary, globs, embeddings)
knowledge/entities/
  people/{jane-doe,amir-khan}.md        person:…
  departments/{platform,logistics,hr}.md  department:…
  methods/event-sourcing.md            method:…
  systems/ingest-api.md                system:…
  events/2026-kickoff.md               event:…
projects/atlas/2026_q2/
  README.md                            project:atlas-2026-q2  (the project entity itself)
  STATUS.md                            record:…
  decisions/2026-06-08-ingest-decision.md
  meeting-notes/2026-06-10-broker-sync.md
```

## The model in one screen

- **Address = `type:id`.** Each node's frontmatter carries a bare `id:` slug plus a
  `type:`; they compose to the address. `id: hr` + `type: department` ⇒ `department:hr`.
- **References are IDs, never names.** `[[department:hr]]` renders to
  `[Human Resources](./hr.md)` — display text from the target's `name:`; a pipe overrides
  it: `[[department:hr|HR]]` → `[HR](./hr.md)`.
- **Frontmatter edges + inline wikilinks.** Frontmatter fields whose values parse as a
  `type:id` are edges keyed by the field name (`participants`, `org`, `owner`,
  `implements`, `project`, …); inline `[[…]]` are `inline` edges.
- **Unresolved loose ends.** `[[?person: someone from logistics]]` and
  `[[?: the broker thing]]` in the broker-sync note are *not* edges — they are the work
  list for the entity-creation pass (`vaire unresolved`).

## Try it

`vaire` finds the corpus root by walking up to a `.vaire/` directory (this folder has one),
so you can index it in place. Run from this directory:

```bash
vaire --repo . index                          # build .vaire/index.db
vaire --repo . status
vaire --repo . check                          # clean: no duplicate/dangling, no orphans
vaire --repo . resolve department:hr          # → Human Resources
vaire --repo . backlinks system:ingest-api    # who points at the Ingest API
vaire --repo . refs record:2026-06-10-broker-sync --depth 2
vaire --repo . unresolved                     # the two [[?…]] loose ends
```

Since this folder is not its own Git repo, `vaire` indexes the working tree directly and
records `commit: null`. (Inside a real Git corpus, it would index the committed tree —
commit-as-publish.) Everything resolves, so `vaire check` is clean; `vaire unresolved`
reports exactly the two descriptors above.
