---
name: vaire-files
description: Explains how a Vairë knowledge corpus is structured and authored. Nodes are Markdown files whose frontmatter carries a bare `id:` plus a `type:` that compose to an addressable `type:id`; they reference each other with `[[type:id]]` wikilinks, or `[[?type: descriptor]]` when the target is not yet known. Use this when creating, editing, or interpreting files in a Vairë corpus — writing entities or records, adding references between them, resolving unresolved references, or understanding frontmatter edge lists, display names, supersession, scoped IDs (records scoped under a container via `scope:`, addressed as `container-id/type:local`), and the additive authoring rules.
metadata:
  project: vaire
---

# Vairë corpus files

A Vairë corpus is a Git repository of Markdown. Every meaningful file is a **node**.
There are two kinds, distinguished only by convention (the engine is type-agnostic):

- **Entities** — things with identity that get referred to: people, departments,
  methods, systems, events. **Global** (one entity, referenced from anywhere). Usually
  have a `name:`.
- **Records** — things that happened or were produced: meeting notes, decisions, status.
  **Project-scoped** and **immutable / additive** — never rewrite a record's prose; write
  a new record.

## Node identity: `type:id`

A file is a node if (and only if) its frontmatter has **both** an `id:` and a `type:`.
The node's address is the composition **`type:id`**.

- `id:` is the bare, locally-unique slug (`hr`, `jane-doe`, `2026-06-10-broker-sync`).
- `type:` is the authoritative type **and** the ID namespace (`department`, `person`,
  `record`, …).
- So `id: hr` + `type: department` ⇒ the node is addressed as `department:hr`.

Discovery is by frontmatter, **not** by file path — files can move freely; references are
always IDs, never paths. The directory layout is advisory.

### Project-scoped record IDs (if enabled)

If the corpus config lists a type in `scoped_types` (commonly `record`), a node of that
type with a `project:` is addressed as a **path of typed IDs**:
**`<project-id>/<type>:<local-id>`**. You write only a short **local** id; the scope is the
node's own `project:`:

```markdown
---
id: 2026-06-10-standup            # local — unique within the container only
type: record
scope: project:atlas-2026-q2      # the container; its value's type can be anything
---
```
⇒ address `project:atlas-2026-q2/record:2026-06-10-standup`. The node's own ID is the last
segment (`record:2026-06-10-standup`); the `project:atlas-2026-q2/` prefix is the scope.
Reference it fully as `[[project:atlas-2026-q2/record:2026-06-10-standup]]`, or — from
another node in the same container — relatively as `[[record:2026-06-10-standup]]`.
Cross-container references must be written in full. The container can be any type
(`scope: org:some-firm` → `org:some-firm/record:…`); the `scope:` value is also a graph
edge to that container. (When `scoped_types` is empty, IDs are flat `type:slug` and you give
records globally unique slugs yourself.) The scope-supplying field is configurable
(`scope_field`, default `scope`). Use `vaire resolve` / `vaire search` for the canonical ID.

## Frontmatter

```markdown
---
id: hr
type: department
name: Human Resources
aliases: [HR, People Ops]
status: active
updated: 2026-06-15
---
# Human Resources

Owns onboarding and headcount.
```

```markdown
---
id: 2026-06-10-broker-sync
type: record
project: project:atlas-2026-q2
participants: [person:jane-doe, department:logistics]
references: [method:event-sourcing, system:ingest-api]
---
# Broker sync, 2026-06-10

[[person:jane-doe]] walked [[department:logistics]] through partition segmentation.
```

- **`name:`** is the display name (see Rendering below). It's **optional** — if omitted,
  the display name falls back to the sole `# H1` heading in the prose, else the filename
  without extension.
- **`project:`** on a record is its scope — load-bearing, the only source of scope.
- **Frontmatter is the structured edge list.** A field whose value is a `type:id` — where
  `type` is a **configured type** (config `id_prefixes`) — becomes a graph edge keyed by
  that field name (`participants`, `references`, `org`, `owner`, `implements`, `scope`, …).
  You may invent field *names*; reference *types* must be in the vocabulary, so a colon in a
  title/note isn't mistaken for a reference. `name`/`aliases` are display fields, never refs.
- **Frontmatter references are bare — no `[[ ]]` brackets** (brackets are a prose-only
  convention; in frontmatter they're a silent trap that `vaire check` flags as
  `frontmatter_wikilink`). A field can also hold a **not-yet-created** reference using the
  bare unresolved form, quoted for the leading `?`: `head: "?person: someone senior"` — it
  shows up in `vaire unresolved` and is *not* an edge. So: `head: person:jane` (resolved)
  or `head: "?person: …"` (unresolved), never `head: [[…]]`.
- `id`, `type`, and `superseded_by` are never edges.

## References

References are **IDs, never bare display names.** Four forms:

```
[[person:jane-doe]]                      resolved (ID known)
[[person:jane-doe|Jane]]                 resolved, with display text
[[?person: someone from logistics]]      unresolved (type hint + descriptor)
[[?: the broker thing]]                  unresolved, type unknown
```

**The rule:** a `?` immediately after `[[` means **unresolved** — everything after the
colon is a *descriptor* (what you saw), never an ID, and never a graph edge. No `?` means
a real reference to a real node.

Unresolved references carry a **description, never a guessed name or slug**. Do not invent
an ID. Two authors guessing slugs for the same thing is the duplicate-entity problem;
descriptors don't collide.

## The authoring contract (follow exactly)

Every author — human or agent — follows the same three steps when referencing something:

1. **Look it up in the index first** — `vaire suggest "<descriptor>" --type <T>` ranks the
   existing IDs that descriptor might be (the purpose-built lookup-before-reference tool);
   `vaire resolve <type:id>` confirms one. (See the `vaire-query-cli`/`vaire-query-mcp` skills.)
2. **Found it → write `[[type:id]]`.**
3. **Didn't find it, or unsure → write `[[?type: descriptor]]`. Never mint an entity
   inline.**

Two invariants:

- References are IDs, never bare display names.
- Records are additive — never rewrite an existing record's prose; write a new record.

The one sanctioned in-place edit is **resolving** an unresolved reference, which is
additive (add the ID, keep the original phrasing as display text):

```
[[?person: someone from logistics]]  →  [[person:logistics-contact|someone from logistics]]
```

Creating a brand-new entity is **not** your call when authoring — it is a separate, gated
step. Just leave a `[[?...]]` descriptor.

## Display & rendering

A resolved reference renders to a portable Markdown link, with link text from the
target's `name:` (or a `|` override):

```
[[department:hr]]      →  [Human Resources](./hr.md)
[[department:hr|HR]]   →  [HR](./hr.md)
```

This is why references are IDs: renaming an entity (editing one `name:`) updates the
display of every reference to it. `vaire render <id>` produces this resolved Markdown.

## Supersession

If a duplicate or wrong entity exists, do not delete it — add `superseded_by: <id>` to the
loser's frontmatter. The index follows the redirect, so existing references to the old ID
still resolve.

## Don't

- Don't mint an entity / invent an ID inline — use `[[?type: descriptor]]`.
- Don't rewrite an existing record's history — add a new record.
- Don't reference by display name or file path — reference by `type:id`.
