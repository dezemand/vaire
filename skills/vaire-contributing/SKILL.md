---
name: vaire-contributing
description: >-
  The workflow for writing new knowledge into a Vairë corpus as a contributor — the autonomous-
  safe role, and the default role for an agent that learned something. Use this when capturing
  meeting notes, decisions, status, or observations as records; extending an entity's prose with
  new confirmed material; adding aliases; raising or resolving `[[?...]]` loose ends; or
  validating and publishing such changes. Covers what a contributor may and must not change, the
  edit → validate (`--working-tree`) → commit loop, commit-as-publish, and the hard gates (no
  entity creation, no record rewrites, no deletions, no version bumps).
metadata:
  project: vaire
---

# Contributing to a Vairë corpus

You are in the **contributor** role when you have learned something and want the corpus to
know it. This is the autonomous-safe role: everything a contributor may do is **additive
and locally contained**, which is exactly why it needs no human gate. Roles are defined by
what they may change, not by who holds them — an agent is almost always a contributor (or
an observer — see the **vaire-answering** skill), never a maintainer.

## What you may change

| act | form |
|---|---|
| Write a **record** | a new file — meeting notes, a decision, status; never edit an existing record's prose |
| **Extend entity prose** | append new confirmed material to an existing entity (a new fact, a new `##` facet) |
| Add **`aliases:`** | a term people actually use for an existing entity — this seeds future resolution |
| Add **references** | `[[type:id]]` links in prose, edges in frontmatter |
| Raise a **loose end** | `[[?type: descriptor]]` for anything you couldn't resolve |
| **Resolve** a loose end | the one sanctioned in-place edit — add the ID, keep the phrasing as display text (example below) |

```text
[[?person: someone from logistics]]  →  [[person:logistics-contact|someone from logistics]]
```

## What you must not do — the hard gates

- **Never create an entity.** Creating identity is the one gated, maintainer-run process
  (see the **vaire-entity-creation** skill). Not finding an entity is not a blocker: write
  `[[?type: descriptor]]` and move on — that *is* the correct completion of your task.
- **Never rewrite a record.** Records are immutable history; new information is a new
  record.
- **Never delete anything.** Wrong or duplicate material is a report to the maintainer,
  not a deletion.
- **Never bump versions or edit the manifest** — except `vaire add` when your change
  introduces the package's first reference to a dependency, declared in the same change.
- **Never edit another package's files.** Something wrong in a dependency is a report to
  *that* package's maintainer; your own package can at most carry a record noting it.

## The authoring contract

Before writing any reference (full model in the **vaire-files** skill):

1. **Look it up**: `vaire suggest "<descriptor>" --type <T>` (falls back to `vaire search`
   for content-shaped questions).
2. **Found it** → `[[type:id]]`; found it in a dependency → `[[@pkg/type:id]]` (declared —
   see the **vaire-packages** skill).
3. **Not found, or unsure** → `[[?type: descriptor]]`. The descriptor is what you actually
   observed ("the procurement content owner"), never a name or slug you invented — two
   authors guessing slugs is the duplicate-entity problem; descriptors don't collide.

References are IDs, never bare display names. In frontmatter, references are bare and
unbracketed (`participants: [person:jane-doe]`); `[[ ]]` is prose-only.

## A record, end to end

```bash
vaire suggest "jane from the broker team" --type person     # → person:jane-doe
vaire suggest "the ingest service" --type system            # → system:ingest-api
```

```markdown
---
id: 2026-06-10-broker-sync
type: record
scope: project:atlas-2026-q2
participants: [person:jane-doe, department:logistics]
references: [system:ingest-api]
---
# Broker sync, 2026-06-10

[[person:jane-doe]] walked [[department:logistics]] through partition segmentation of
[[system:ingest-api]]. Rollout owner still unknown: [[?person: the rollout owner on the
broker side]]. Decision: shard by broker id, revisit 2026-Q3 if volume doubles.
```

Qualifiers — dates, owners, numbers, conditions ("revisit if volume doubles") — are the
knowledge; never summarize them away. No personal names in prose beyond references: a
person is an edge to a `person:` entity or a `[[?person: …]]` loose end, never inline
contact details.

## Validate, then publish

**Commit = publish.** Until then, work is invisible: keep it uncommitted, or park longer
explorations under `drafts/**` (excluded from the index by default).

The edit loop, without committing anything:

```bash
vaire check --working-tree      # reindexes your uncommitted edits, then runs all guards
```

Fix what it reports (each finding's sanctioned fix is in the **vaire-check-triage**
skill) and re-run. **A clean `vaire check` is the definition of done** for any change.
Then commit, with a message that states the source of the knowledge — the commit is the
provenance record. Your merged commit is the confirmation; until a maintainer (or an
authorized human) merges, your output is a proposal.

## Don't

- Don't mint entities, IDs, or packages — descriptors (`[[?…]]`) are the correct output
  for the unknown.
- Don't "fix" existing records, even typos in substance — supersede with a new record.
- Don't block on ambiguity a maintainer should adjudicate — capture it precisely as a
  loose end or a record, and finish.
