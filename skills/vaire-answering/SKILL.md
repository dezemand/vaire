---
name: vaire-answering
description: >-
  How to answer questions from a Vairë corpus as an observer — read, cite, change nothing. Use
  this when asked a question the corpus should answer ("who owns X", "how does process Y work",
  "what do we know about Z"): choosing the right lookup (`suggest` for entities, `search` for
  content, `backlinks`/`refs` for structure), reading nodes for depth, citing answers by node
  address (`type:id`, `@pkg/type:id`), keeping corpus facts distinct from your own inference,
  and reporting wrong or missing knowledge instead of fixing it.
metadata:
  project: vaire
---

# Answering questions from a Vairë corpus

You are in the **observer** role: you read the corpus and change nothing. The query
surface is the same from a shell (**vaire-query-cli** skill) or over MCP
(**vaire-query-mcp** skill); this skill is about using it well and reporting honestly.

## Pick the right first move

| the question is… | start with |
|---|---|
| "the *thing* called/known as X" — an entity lookup | `suggest "X"` (name/alias match first), then `resolve` the winner |
| "what do we know about X" — content-shaped | `search "X"` (hybrid full-text + vector; `--type`, `--scope` to narrow) |
| "what points at X" / "how load-bearing is X" | `backlinks <id>` |
| "what does X build on / consist of" | `refs <id>` (`--depth 2` to traverse) |
| "what's around this project" | `search --scope project:<id>`, then follow links |

Results are **pointers** (IDs, paths, section anchors) — open the file or `render <id>`
for the actual content. Don't answer from snippets alone when the question has any depth;
the first paragraph of an entity is written to answer "what is X?", so read it.

Traversal crosses package boundaries: dependency hits arrive as `@pkg/type:id`, and
fan-out reads list unavailable dependencies under `skipped` — if something relevant was
skipped, say so in your answer rather than treating the sweep as complete.

## Citing — the observer's obligation

**Cite what you use by address**: `type:id`, or `@pkg/type:id` for a dependency's node
(plus a section heading when it matters). Addresses are stable; paths and display names
are not.

**Keep provenance distinct.** A corpus fact is citeable to a node; anything you inferred —
a synthesis, a gap-filling assumption, a judgment — is *your* claim, not the corpus's.
Never present the two in one undifferentiated voice:

> Ingest is owned by [[department:logistics]] (`system:ingest-api`). Nothing records a
> deputy owner — if one exists, the corpus doesn't know it.

Two honesty rules the format gives you for free:

- A `[[?…]]` loose end in a node means **the corpus knows that it doesn't know** — report
  the descriptor as an open question, never resolve it yourself by guessing.
- `updated:` and `vaire status` tell you how fresh knowledge is; the index reflects the
  last `vaire index` run, which may lag recent commits. Flag staleness when the question
  is time-sensitive.

## When the corpus is wrong or missing something

An observer does not fix it — not even a typo. Report it to a contributor or the package
maintainer. The cheapest useful report is precise:

- the node address (`type:id`),
- what you expected,
- what you found (or didn't).

If you are *also* authorized to write, switch roles deliberately and follow the
**vaire-contributing** skill — capturing what you learned as a record or loose end —
rather than editing ad hoc mid-answer. Entities are never yours to create or correct in
passing.

## Don't

- Don't answer from memory what the corpus can answer from a node — look it up, cite it.
- Don't present inference as corpus fact, or corpus fact without an address.
- Don't treat an empty `search` as "the org doesn't have this" — say the *corpus* doesn't
  record it; check `unresolved` to see whether it's a known gap.
- Don't edit anything, and don't guess at loose ends.
