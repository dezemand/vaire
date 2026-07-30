---
name: vaire-entity-creation
description: >-
  The entity-creation pass — the one gated Vairë process, which turns accumulated `[[?...]]`
  loose ends into links to existing entities or into new ones. Use this when asked to run the
  pass or to "create the entities": processing `vaire unresolved` output, clustering
  descriptors, deduplicating against existing entities, adjudicating "new entity or that one?",
  resolving references, and superseding duplicates with `superseded_by:`. This is maintainer
  work performed on request — never a side effect of authoring; an agent running it surfaces
  genuinely ambiguous identity decisions instead of guessing.
metadata:
  project: vaire
---

# The entity-creation pass

Records are written autonomously and freely; **entity creation is the single act pulled
out of the autonomous path**, because it is the only irreversible, propagating operation —
IDs are referenced everywhere, so a duplicate or mis-merged entity poisons every
referencing record. The asymmetry is deliberate: the frequent act (writing what you
learned) is free; the rare act (creating identity) is gated. Run this pass only when a
maintainer asks for it (or you hold that role for the package).

**The worklist is the corpus itself.** The pass's input is the `[[?...]]` references
currently in the files, found by one scan — no queue, no state to desync, crash-safe: if
the pass dies, rerun it.

## The algorithm

### 1. Scan

```bash
vaire unresolved --json          # every loose end: {record, path, type_guess, descriptor, line}
```

Filter with `--type` to work one type at a time — identity judgments are easier within
one kind.

### 2. Cluster, type-gated

Group descriptors that plausibly mean the same thing. **Type is a hard filter** — a
`?person` never clusters with a `?department`; similarity ranks only within a type. So
"someone from logistics" / "the logistics contact" collapse into one candidate.

### 3. Match each cluster against existing entities

```bash
vaire suggest "the logistics contact" --type person
```

Aliases and full-text hit first, embeddings are backup — a descriptor usually matches an
entity's `aliases:` far more reliably than a vector. Candidates include dependency
entities, arriving pre-qualified as `@pkg/type:id` (referencing one requires the declared
dependency — **vaire-packages** skill).

Then adjudicate:

- **One clear existing match** → resolve every reference in the cluster to that ID. No
  new entity.
- **No plausible match, one coherent cluster** → create one new entity. The clustered
  descriptors are the raw material: five records describing it five ways are five lines
  of its description, and they **seed its `aliases:`** so the next descriptor resolves by
  alias instead of guesswork. Format per the **vaire-entity-authoring** skill — the
  admission tests apply; a descriptor cluster that can't clear the substance floor stays
  unresolved rather than becoming a stub.
- **Genuine ambiguity** — a cluster matching two existing entities, or two clusters that
  might be one — **is surfaced to a human, never guessed.** Present the candidates and
  the evidence; identity merges are cheap to ask about and expensive to get wrong.

Entities are low-volume by nature (references are reused constantly; identities are
created seldom), so what reaches the human is a trickle of "new entity, or that one?" —
keep it that way by auto-resolving only the unambiguous cases.

### 4. Resolve the references

Rewriting is **additive**: add the ID, keep the original phrasing as display text —

```text
[[?person: someone from logistics]]  →  [[person:logistics-contact|someone from logistics]]
```

Frontmatter loose ends lose the `?` form and become real edges:
`owner: "?department: procurement content owner"` → `owner: department:procurement`.
This is the one sanctioned in-place edit of a record.

### 5. Validate and publish

```bash
vaire check --working-tree       # new entities admissible, no danglers, no duplicates
```

Fix per the **vaire-check-triage** skill, then commit — one commit per coherent batch,
message naming what was created and what was linked. Commit = publish.

## Repairing a wrong identity: supersession

Autonomous linking can still pick wrong, and a duplicate can slip in before dedup runs.
The losing entity is **superseded, never deleted**:

```yaml
superseded_by: person:jane-doe        # in the loser's frontmatter; @pkg/… allowed
```

The index follows the redirect, so every existing reference to the old ID still resolves.
Deletion without a tombstone is the one way to truly break references — `vaire check`
treats it as an error.

## Don't

- Don't create an entity for a lone descriptor you merely *could* name — no match and no
  cluster substance means it stays a loose end.
- Don't guess between two plausible existing matches — surface it.
- Don't invent slugs disconnected from the descriptors, and never auto-suffix a collision.
- Don't resolve a descriptor by rewriting its phrasing away — the phrasing is evidence,
  keep it as display text.
- Don't run this pass opportunistically while doing contributor work — leave loose ends
  (**vaire-contributing** skill) and let the pass be its own deliberate step.
