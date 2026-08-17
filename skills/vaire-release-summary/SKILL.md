---
name: vaire-release-summary
description: >-
  How to write the narrative summary for a Vairë release — the prose `vaire release
  --summary <file>` carries into the release record. Use this when asked to draft release
  notes or a changelog for a package, when running as the summarising step of a release
  pipeline, or when a `vaire release --dry-run --json` plan has been handed to you to turn
  into prose. Covers reading the plan, researching what actually changed, the frontmatter
  a summary may and may not set, and the rules that decide whether the release will accept
  what you wrote.
metadata:
  project: vaire
---

# Writing a release summary

`vaire release` computes everything structural about a release — the version, the bump,
and the lists of what was added, changed, retired and removed. What it cannot compute is
**what any of that meant**. That is this job: one file of Markdown, handed to
`vaire release --summary <file>`, which lands as the `## Summary` section of the release
record.

Vairë never calls a model. You are a separate step, and your entire interface is a file.

## What you are given

A plan, from `vaire release --dry-run --json`:

```json
{
  "package": "acme-core", "status": "planned", "version": "1.5.0", "bump": "minor",
  "added": ["system:ingest"], "changed": ["department:platform"],
  "retired": [], "removed": [],
  "advisories": [{ "id": "department:platform", "inbound": 14 }]
}
```

`status` decides whether there is work at all:

| status | what it means | what you do |
|---|---|---|
| `planned` | a release is ready to cut | write the summary |
| `nothing` | no entity changed since the last release | stop; write nothing |
| `blocked` | the classifier saw a MAJOR nobody has consented to | stop; a maintainer must pass `--major` |

A `planned` plan carrying `"notes_required": true` is a MAJOR that still owes its
**invalidated-assumptions notes** — a separate file, passed as `--notes`, addressed at the
end of this skill.

## Research before you write

The lists are addresses, not knowledge. Read the entities before describing them — you
have the checkout and the `vaire` binary, so use the corpus rather than guessing from
slugs:

```bash
vaire render system:ingest              # what the entity now says
vaire backlinks department:platform     # who depends on the thing that changed
vaire refs system:ingest                # what it points at
git log -1 --stat                       # what the commits actually did
```

`advisories` is where to spend your attention: an entity with many inbound references
changed, so a lot of readers are affected by whatever changed about it. Say what changed
for them.

## What to write

Prose for someone deciding whether this release affects them. A few paragraphs at most.

- **Lead with consequence, not inventory.** The record already lists every address under
  *Added* / *Changed* / *Retired*; repeating them as sentences adds nothing. "Ingestion
  moved in-house" beats "system:ingest was added".
- **Reference entities as `[[type:id]]`** in the prose. Those become real graph edges, so
  a reader arriving from an entity's backlinks finds the release that touched it. Use only
  addresses that **exist** — see the hard rules below.
- **Say what a consumer should do**, when there is anything to do.
- **Never invent.** If the diff does not tell you why something changed, describe what
  changed and stop. A confident wrong sentence about a knowledge corpus is worse than a
  thin one.
- **No heading.** The `## Summary` heading is added for you; start at prose. Sub-headings
  inside are fine for a large release.

## Frontmatter you may set

Optional, and only the keys that are yours:

```markdown
---
name: "Ingestion in-house"
summary_by: "<model or tool that wrote this>"
---
Ingestion moved in-house: [[system:ingest]] now owns what [[department:platform]]
used to do by hand. Anyone reading the platform entity for ingestion behaviour should
follow the reference instead — the routing rules moved with it.
```

- `name` retitles the record (the version stays its address and stays in the headline).
- `aliases` are **added to** the version spellings, never replace them.
- Any key of your own rides along — `summary_by` is the conventional attribution.

**Refused, by name:** `id`, `type`, `date`, `bump`, `added`, `changed`, `retired`,
`generated_summary`. These are the release's claims about itself, and they are computed.
Setting one fails the release rather than being ignored, so you cannot quietly disagree
with the classifier.

Frontmatter references are **bare** — `related: system:ingest`, never
`related: [[system:ingest]]`. The bracketed form in frontmatter is a warning that does not
fail the release, so it will not stop you: it will just silently fail to become an edge.

## The two rules that decide whether your work is accepted

1. **Every address you name must exist.** After the record is written, `vaire release`
   re-runs `vaire check` with it in place. A reference to something that is not a node
   refuses the whole release, names the address, and rolls the record back. Verify with
   `vaire resolve <id>` before you write it — including addresses you copied from the
   plan's `removed` list, which by definition no longer resolve. Mention removed entities
   as `` `code` ``, never as references.
2. **The prose is not the record.** You cannot change the version, the bump, or the entity
   lists. If the classification looks wrong to you, say so to whoever is running the
   release — do not try to write around it.

Check your own work before handing the file over:

```bash
vaire release --dry-run --summary release-summary.md --json
```

That rehearses everything the real release does — including writing the record and
checking it — and leaves the tree untouched. If it refuses, fix the summary and run it
again.

## Drafting MAJOR notes

When the plan says `"notes_required": true`, a second file is owed: the **invalidated
assumptions**, passed as `--notes`. It answers one question for a dependent — *what did
you believe that is no longer true?* — and it is what they read to decide whether their
own references still hold.

Draft it from the `removed` and `retired` lists, naming each and what replaces it. Two
things to hold onto: a removed address has no target left, so name it as `` `code` ``; and
this is a claim about **meaning**, so a maintainer approves it. Draft, hand it over, and
let them decide — the release refuses without it either way.

## Related

- **vaire-versioning** — what MAJOR/MINOR/PATCH mean and why the version is computed.
- **vaire-query-cli** — `render`, `backlinks`, `refs`, `resolve` in full.
- **vaire-check-triage** — what a `check` finding means, if the gate refuses you.
