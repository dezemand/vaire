---
name: vaire-entity-authoring
description: >-
  What a good Vairë entity file looks like, and the bar for admitting one at all. Use this when
  writing, converting, or reviewing entity files — during the entity-creation pass, when
  building a package from source material (documents, wikis, transcripts), or when auditing
  existing entities. Covers the admission tests (single subject, mappable type, substance floor,
  standalone), the canonical file shape (frontmatter edges, rich aliases, boundary-first body,
  facet sections), slug discipline, rewriting rules (extractive-first, qualifiers survive, PII
  becomes edges never prose), and provenance scalars.
metadata:
  project: vaire
---

# Authoring Vairë entities

An entity file is **one subject, standalone, reference-dense**. This skill is the quality
bar; the file mechanics are in the **vaire-files** skill, and *whether you may create an
entity at all* is governed by the **vaire-entity-creation** skill — entity creation is
gated maintainer work, never a side effect of authoring.

## Admission — is this an entity at all?

Source material is quarry, not units: one document may yield several entities, several
documents one entity, and discard is legitimate. Four tests, all required:

1. **Single subject.** It answers "what is X?" for one X, with a one-sentence boundary
   and at least one thing that is out of scope. If you cannot say what it is *not*, the
   subject is not yet one subject.
2. **A type from the vocabulary.** It must map to a configured `type:`. Reuse what
   exists; a genuinely new kind of thing is an ontology decision for the maintainer (the
   ladder in the **vaire-package-curation** skill), never a silent invention. No type →
   no entity.
3. **Substance floor.** If the honest content is two sentences, it is a `##` section of
   its parent or an `aliases:` entry — not a file. **No stubs**: a stub is a dangling
   promise with an ID, and IDs are forever.
4. **Standalone.** No deixis ("as above", "this page", "click here", "contact us"), no
   unresolved acronyms (expand or wikilink), actors by role or by reference.

## The canonical shape

```markdown
---
id: ingest-api                            # bare slug — address composes as system:ingest-api
type: system                              # the authoritative type AND the ID namespace
name: Ingest API                          # display name; every [[system:ingest-api]] renders with it
aliases: [ingestd, the ingest service]    # what people actually call it — seeds resolution
owner: department:logistics               # edges: frontmatter values matching type:id
platform: "@acme-core/team:platform"      #   cross-package edges quoted (@ is YAML-reserved)
feeds: [system:billing, system:reporting] #   local edges bare
updated: 2026-06-15                       # freshness; from the source's review date if any
source_url: https://wiki.acme.example/ingest   # provenance scalars ride along untouched
---
# Ingest API

**Event ingestion service** — one-paragraph boundary: what this is, for a reader with no
context, with [[department:logistics]] linked at first mention. What it is not: it does
not transform events (that is [[system:billing]]'s side).

## Interfaces
…

## Operational constraints
…
```

### Frontmatter rules

- **`id:` + `type:` make it a node.** The `id:` is the bare local slug — lowercase,
  digits, hyphens; no type prefix. **Slugs are chosen once and never recycled**: derive
  from the subject, disambiguate with a domain prefix when needed (`web-way-of-working`),
  and on collision let a human decide — never auto-suffix.
- **`aliases:` are load-bearing**, not decoration: `vaire suggest` and the
  entity-creation pass match descriptors against them first, so a rich alias list —
  abbreviations, old names, translations, nicknames — is what prevents the next
  duplicate. Harvest them from the source material.
- **Edges** are frontmatter values matching the reference grammar — bare for local
  (`owner: department:logistics`), quoted for cross-package. Never `[[ ]]` in
  frontmatter. An unknown target is the quoted loose-end form:
  `owner: "?department: the procurement content owner"`.
- **Everything else is a scalar** the index preserves: `updated:`, provenance
  (`source_url`, `content_hash`, `provenance:`), conventions (`status: candidate`,
  `kind:`). A `kind:` facet is also the sanctioned way to express a *variant* without a
  new type.
- **Retiring:** add `superseded_by: <type:id>` (cross-package allowed) and strip the
  rest. Never delete without a tombstone.

### Body rules

- **Exactly one H1 = the name** (it is also the display-name fallback).
- **The first paragraph is the boundary** — what this is, readable with no context,
  including what it is not. Open-ended search returns this paragraph first: write it as
  the answer to "what is X?".
- **`##` sections are facets** (Interfaces / Approach / Steps — whatever the type's
  natural facets are). Sections are the embedding unit: a section that mixes two topics
  retrieves as neither.
- **Link at first mention**, inline, with `[[type:id]]` — the wikilink *is* the
  dependency. Pipe display text only when the sentence needs it.
- **Qualifiers survive.** Dates, owners, versions, numbers, conditions, exceptions *are*
  the knowledge — never summarize them away. Prefer "why / when it applies / what would
  change the decision" over restating the what.
- **No PII in prose.** A person is an edge to a `person:` entity or a `[[?person: …]]`
  loose end — never an inline name, email, or phone number.

## Rewriting from sources

Converting existing material (a wiki page, a spec, a transcript) into entities:

- **Extractive-first.** Restructure, dedupe, normalize, resolve pronouns — but add no
  claims. Anything that would require inference is dropped, not written.
- **Resolve people to entities, then strip the name.** A contact or author becomes an
  edge to the owning `person:`/`department:` node (or a loose end); residual names,
  `mailto:` addresses, and contact details are removed from prose.
- **Provenance: git is primary** — the commit message states the source and is the audit
  record. The source trace rides as scalar frontmatter (`source_url`, `content_hash`,
  `fetched_at`, `provenance:`); a multi-source entity lists all sources.
- Every reference follows the lookup-first contract (**vaire-files** skill); every
  unknown becomes a descriptor, so the conversion's loose ends land in `vaire unresolved`
  as the follow-up worklist.

## Review checklist

Before an entity is done: admission tests pass · boundary paragraph answers "what is X?"
and names an exclusion · aliases harvested · edges declared in frontmatter and linked at
first mention in prose · qualifiers intact · no PII in prose · provenance scalars carried
· `vaire check --working-tree` clean (fixes per the **vaire-check-triage** skill).
