---
title: Vairë — Knowledge Corpus & Index Design Spec
status: draft
date: 2026-06-15
scope: Addressable knowledge corpus and its derived index, for human and autonomous-agent authors
---

# Vairë — Knowledge Corpus & Index Design Spec

## 1. Purpose

A knowledge corpus authored as flat Markdown works well until it needs to refer to
things. References to people, departments, and projects break constantly when they are
stored as display names — names change, nothing is addressable, and there is nothing
stable to resolve against. The fix is **stable entity IDs plus records that reference
them by ID**. That single change is the core of everything below.

The second force shaping the design is **autonomous authoring**. Agents should be able
to write into the corpus with little or no human-at-commit review. Autonomy removes the
human backstop that otherwise keeps a corpus trustworthy, so the system must let agents
write freely *without being able to corrupt what is trustworthy.* The answer is not
"instruct the agent to be careful" — it is to make the one irreversible act structurally
unavailable to the autonomous path.

## 2. Organizing principle

One question governs every placement decision:

> **Does the agent need this to work better, or does something outside need to trust it?**

- Agent needs it to work better → **memory** (owned, disposable).
- Something outside needs to trust it → **corpus** (authored, authoritative).

Memory is an agent's private learned signal; delete it and the agent relearns. The
corpus is authored truth; other people, tools, and agents depend on it being correct.
Everything else follows from keeping these two clean and defining the one boundary
between them.

## 3. The two systems

### 3.1 Memory — owned, disposable (out of scope)

An agent's native memory — running logs, a consolidation pass, a promoted summary store
— is the agent's private scratch and accumulated signal. It is **out of scope for this
spec.** A typical implementation is a continuous-capture / batched-promotion loop:
signals are staged continuously and only promoted to durable memory when gated on
objective retrieval signals (frequency, relevance, recency, consolidation). A single
global memory per agent is fine here, because nothing in it is trust-bearing.

This spec deliberately does **not** specify a memory engine (staging stores, vector
databases, a separate consolidation service). That belongs to the agent runtime. This
spec specifies the corpus and the boundary memory must respect.

### 3.2 Corpus — authored, authoritative (the work)

A Git repository of Markdown, structured around addressable entities. Files are the
source of truth; a derived index makes them queryable. This is what this spec builds.

### 3.3 The boundary between them

This is the load-bearing rule, stated three ways so it does not erode:

- The corpus is a **workspace the agent reads**, not data loaded into the memory engine.
  Memory consolidation runs over *interaction history* (sessions, logs) — **never over
  the corpus.**
- The corpus is **read-as-truth, never promoted-from.** An agent must not treat authored
  corpus facts as promotion candidates competing with its own learned signal. Collapsing
  this is how the two trust levels contaminate each other.
- At read time the agent draws from both but keeps **provenance distinct in its output**:
  a corpus fact is citeable to a record; a memory signal is the agent's own hunch.
  Keeping these labeled at read-time matters as much as keeping the stores separate at
  write-time.

The membrane between memory and corpus is a single deliberate act: an author (human or
agent) writes a corpus record. There is no separate capture store and no promotion
pipeline between the two (see §3.4).

### 3.4 Non-goals (explicitly cut)

Listed so they do not get re-added.

- **No third "captures" tier / capture store.** Records are authored directly; the only
  loose ends that exist live *inline inside records* as unresolved references (§6). There
  is nothing sitting between memory and corpus.
- **No promotion queue, candidate table, or graduation state machine.** Committing a
  record *is* publication (commit-as-publish). Scratch-vs-durable comes from Git
  semantics, not a separate system.
- **No autonomous reference-resolution pipeline with confidence scoring against a human
  adjudication queue.** Resolution is part of authoring plus one gated pass (§8), not a
  standing background service.
- **The corpus is not a database.** Files are authoritative; the index is a disposable
  derived cache.

## 4. Corpus model — layers as stages of referential resolution

The corpus has two kinds of thing. They are not storage tiers; they are stages of how
*resolved* a thing's references are.

**Entities (foundation).** Things with identity that get referred to: people,
departments, methods, systems, events. The defining property is that *other things point
at them.* Each gets a stable typed ID and its own file. Entities are **global** — a
person referenced from three projects is one entity, not three.

**Records (middle).** Things that happened or were produced, scoped to a project: meeting
notes, decisions, research, status. They **reference entities by ID** and are
**immutable** — additive only. To change what a record says, write a new record; never
edit history. (The one sanctioned in-place change is reference *resolution*, which is
additive — see §6.) Records stay **nested under projects**.

There is no third tier. Unresolved material does not get its own store; it lives as
unresolved references inside whatever record is being written.

### Directory layout (advisory)

This tree is a **human convention, not something Vairë enforces.** Vairë classifies files
by frontmatter (an `id:` slug plus a `type:`), not by path — so files can move freely and the graph
stays intact, because every reference is an ID, never a path (see §9, Discovery). A sane
layout still helps *people* find things, and the entity-creation pass writes new entities
here by config; Vairë simply does not depend on it.

```
knowledge/
  entities/
    people/          # IDs: person:<slug>
    departments/     # IDs: department:<slug>
    methods/         # IDs: method:<slug>
    systems/         # IDs: system:<slug>
    events/          # IDs: event:<slug>
projects/
  atlas/
    2026_q2/
      meeting-notes/
      context/
      decisions/
      artifacts/
      STATUS.md
      README.md
```

Directory names are readable plurals; ID prefixes are the typed singular. Because
discovery is frontmatter-driven, the path no longer carries semantics — `type` and
project scope come from frontmatter, not from where the file sits. A node's **address is
the composition `type:id`**: the `id:` field holds only the locally-unique slug (`hr`),
and the **`type:` field is the authoritative type** (`department`), so the two compose to
`department:hr`. The `type:` field is therefore **load-bearing** (it is the ID namespace
and the node's type), as is `project:` (the only source of a record's scope).

## 5. File shapes

**Entity** — `knowledge/entities/departments/hr.md`:

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

Owns onboarding. Tracks changes via [[method:event-sourcing]].
```

The node's **address is `type:id`** — here `department:hr`, composed from the `type:`
field and the bare `id:` slug. The `name:` is its **display name**: a bare reference
`[[department:hr]]` renders to `[Human Resources](./hr.md)`, taking its text from `name:`;
a piped reference overrides it — `[[department:hr|HR]]` renders to `[HR](./hr.md)` (§6).

**Record** — `projects/atlas/2026_q2/meeting-notes/2026-06-10-broker-sync.md`:

```markdown
---
id: 2026-06-10-broker-sync
type: record
project: project:atlas-2026-q2
date: 2026-06-10
participants: [person:jane-doe, department:logistics]
references: [method:event-sourcing, system:ingest-api]
---
# Broker sync, 2026-06-10

[[person:jane-doe]] walked [[department:logistics]] through partition segmentation.
Decision to scope [[system:ingest-api]] first — see
[[record:2026-06-08-ingest-decision]].
```

Its address is `record:2026-06-10-broker-sync` (`type:id`); references point at other
nodes by their composed `type:id`.

**Record with unresolved references** (the common autonomous-authoring case):

```markdown
# Broker sync, 2026-06-10

[[?person: someone from logistics]] raised throughput concerns about the
[[?: the broker thing]]. [[person:jane-doe]] to follow up.
```

Frontmatter carries the structured edge list (parsed without reading prose). Inline
wikilinks preserve position and meaning — who said what. The same reference can appear in
both. **Frontmatter references are bare** — no `[[ ]]` brackets (those are an inline-prose
convention): a resolved edge is `field: type:id`, and a not-yet-created one is the §6
unresolved form *without* brackets, quoted because of the leading `?`:
`head: "?person: someone senior"`. The latter is recorded as a loose end (it is *not* an
edge), so structured fields can name things that don't exist yet. Writing `[[ ]]` in
frontmatter is the common trap — unquoted it parses to junk, quoted it no-ops — so
`vaire check` flags it (cli.md §6.3); Vairë also strips stray brackets forgivingly.

## 6. Reference syntax

References are **IDs, never bare display names.** A reference is in one of three states.

```
[[person:jane-doe]]                              resolved (ID known)
[[person:jane-doe|Jane]]                         resolved, with display text
[[?person: someone from logistics]]              unresolved (type guess + descriptor)
[[?: the broker thing]]                          unresolved, type unknown
[[person:logistics-contact|someone from logistics]]  resolved-from-unresolved (phrasing kept)
```

**Parse rule.** A `?` immediately after `[[` means **unresolved**. Everything after the
colon is a *descriptor*, not an ID — the index must **not** follow it as a graph edge. No
`?` and it is a real reference to a real entity. The type after `?` is a hint: optional,
and overridable by the entity-creation pass.

**Display name & rendering.** A resolved reference carries *display text*. With no `|`,
the display defaults to the target's **`name:`** field; a `|` overrides it. Rendering a
reference produces a relative Markdown link — the display in the brackets, the target file
as the href (path relative to the referencing file):

```
[[department:hr]]      →  [Human Resources](./hr.md)   display from the target's name:
[[department:hr|HR]]   →  [HR](./hr.md)                piped text wins
```

This is *why* references are IDs and not names: link text is resolved from `name:` at
render time, so renaming an entity (editing one `name:`) updates the display of every
reference to it without touching a single referencing record. The address — `type:id` —
never changes.

**`name:` is optional.** The display name resolves by fallback: the `name:` field, else the
**sole `# H1`** in the prose, else the **filename** without extension. (A missing or
*ambiguous* — not exactly one — H1 skips to the filename.) Vairë resolves this once at index
time and stores it as the node's `name`, so `resolve`/`render` always have one even when the
frontmatter omits it.

**The core decision: unresolved references carry a description of what the author saw,
never a proposed name for it.** No provisional slug. A provisional slug is still a slug,
and two authors guessing slugs for the same referent is the duplicate-entity problem
wearing a question mark. Carry a descriptor and there is nothing to collide; identity gets
created exactly once, in the gated pass. The autonomous path has no syntax for asserting
an identity.

**Resolution is additive.** Resolving an unresolved reference *adds an ID target and
preserves the original phrasing as display text:*

```
[[?person: someone from logistics]]  →  [[person:logistics-contact|someone from logistics]]
```

The record's words never change — a target was added, history was not revised. This keeps
records additive-only (immutability preserved) and keeps the thing inline links exist
for: the speaker said "someone from logistics," and the record still shows that, now
followable. Each resolution is its own commit; the Git diff is the audit trail (which
descriptor resolved to which ID, and who decided), for free.

## 7. Authoring contract

Every author — human or agent — follows the same contract when writing a corpus file.

1. **Referencing an entity? Look it up in the index first.** ("Is there an entity for the
   logistics department?")
2. **Found one → `[[type:id]]`.**
3. **Did not find one, or unsure → `[[?type: descriptor]]`. Never mint an entity inline.**

Plus the two invariants:

- References are IDs, never bare display names.
- Records are additive — never rewrite an existing record's prose; write a new record.

That is the whole contract. The dangerous decision (does a new entity get created) is
simply *not in the author's hands* — there is no rule for it because the author never does
it. That is what makes the contract safe to hand an autonomous writer.

## 8. The entity-creation pass (the one gated process)

Records are written autonomously and freely. The single act pulled out of the autonomous
path is **entity creation**, because it is the only irreversible, propagating operation:
IDs are referenced everywhere, so a duplicate or mis-merged entity poisons every
referencing record. Gate the rare dangerous act; free the frequent safe one. (Records are
additive and locally contained — a wrong record is just a wrong record.)

**Statelessness — why this is not a queue.** The pass's work list is *the `[[?...]]`
references currently in the corpus*, found by one scan. It is derivable from the files at
any time and crash-safe: if the pass dies, rerun it — the loose ends are still sitting in
the records, because they never lived anywhere else. The "queue" is the file system. There
is no candidate table to desync from reality, because the to-do list *is* reality.

**Algorithm.** Almost entirely the retrieval index used in the other direction:

1. **Scan** for unresolved references → `{record, type_guess, descriptor, location}`.
   *(The one genuinely new index capability.)*
2. **Cluster** descriptors by semantic similarity, **type-gated** — only within the same
   `?type`; type is a hard filter, similarity ranks within it. So "someone from logistics"
   / "the logistics contact" collapse, but a `?person` never clusters with a `?department`.
   *(Semantic search, reused.)*
3. For each cluster, **match against existing entities — alias + FTS first, embeddings as
   backup** (a descriptor like "logistics contact" hits `department:logistics` via its
   `aliases:` far more reliably than via a vector). *(The same hybrid search as §9.)*
   - **Match** → link every reference in the cluster to the existing ID.
   - **No match** → create one entity from the clustered descriptors. The descriptions are
     the raw material for the new entity's file (five records describing it five ways are
     five lines of its description), and seed its `aliases:` so the next descriptor
     resolves by alias, not by guesswork.
4. **Rewrite** each reference to its resolved form (additive — add ID, keep phrasing) and
   **commit.**

**Auto vs. surface.** Unambiguous cases resolve automatically: one clear existing match,
or one coherent cluster with no plausible existing entity. Genuine ambiguity is surfaced
to a human reviewer: a cluster that matches two existing entities, or two clusters that
might be one. Because entities are low-volume by nature — references get reused
constantly, entities get created seldom — what reaches the reviewer is a trickle of "new
entity or that one," not a flood of record reviews. That is what makes the human step
**decay-resistant**: there is no flood to fall behind on.

**Threshold note.** The exact auto-vs-surface boundary is deliberately *not* fixed here.
Tune it against the first week of real `[[?...]]` references rather than imagined ones
(§11).

**Supersession (minimal merge convention).** Autonomous linking can still pick a wrong
existing entity, and a duplicate can slip through before dedup runs. When it does, the
losing entity is superseded, not deleted: add `superseded_by: <id>` to its frontmatter.
The index follows the redirect, so existing references to the old ID still resolve. This
is the minimum needed to keep the safety story honest; the merge *UX* is deferred (§11).

## 9. Vairë — the index (derived, read-only)

**Vairë** is the reference graph over the corpus, woven from the files. It holds no truth
of its own — the deeds live in the files — it weaves them into a tapestry that can be
queried across. CLI binary: `vaire`. The CLI surface is specified in [cli.md](cli.md);
this section specifies what the index *is*.

A **Rust + SQLite** engine. It is a derived cache — disposable, rebuildable from the files
in seconds — and it **never writes the corpus.** This is the safe asymmetry: a derived
index cannot corrupt the truth, whereas an authoritative DB would be a liability to back
up and protect.

**Discovery (layout-agnostic).** A file is a Vairë node if its frontmatter carries an
`id:` (a local slug) **and** a `type:`; its address is the composition `type:id`
(`department:hr`) — *not* its path or filename. Everything else is prose Vairë ignores
(optionally FTS-only). This makes Vairë a **generic frontmatter-graph
indexer**: a node is `{id (the composed `type:id`), type, frontmatter, prose, outbound refs}`,
and the entity/record semantics live in the corpus conventions and the authoring contract,
not in the engine — none of the tools below need that distinction. Two integrity checks
fall out of ID-based discovery and run at index time: every composed `id` (`type:id`) must
be **unique** (the duplicate-entity guard), and every non-`?` reference must **resolve**
(dangling refs surface). An optional committed config carries include/exclude globs (skip
`node_modules`, drafts, archives) — that is *where to look*; the `id:`+`type:` pair is *what it is*.

**Storage:** an edges table (`from_id, to_id, ref_type, source_file, line`), an FTS5 index
over prose, and per-section embeddings. SQLite as a graph is plenty at this scale — no
dedicated graph database.

**Schema version.** A one-row `schema_version` table stamps the index with a version
number — the one table whose shape never changes, so any future build can read it to learn
how to migrate the rest. On a schema change the version is bumped; `vaire index` then
rebuilds from scratch (the index is disposable), and a read against a mismatched index is
rejected (exit `3`, "rebuild with `vaire index --full`") rather than querying an unexpected
shape. `vaire status` reports the version.

**Search — FTS first, vectors for recall.** Three retrieval jobs want different things.
*Backlinks and traversal* are pure graph — no vectors. *Reference resolution* (§8) matches
short descriptors to entities, where the `aliases:` list and FTS are high-precision and
bare embeddings are weak ("the broker thing" embeds to mush) — so resolution is **alias +
FTS first, embeddings as backup.** *Open-ended retrieval* is where vectors earn their
place, because the caller's phrasing will not lexically match the records. So vectors are
the recall layer behind FTS-and-aliases, primary only for open search — not the headline
mechanism.

**Embeddings — per section, local, cached.** Sections split on headings (`##`); each chunk
is embedded; the **file is the returned unit**. Vectors live in the *same*
`.vaire/index.db` — either via `sqlite-vec`, or (simpler at this scale) an embedding blob
column with brute-force cosine in Rust; a few thousand sections is sub-millisecond, so ANN
may be unnecessary. Same reasoning as SQLite-as-graph: no separate vector store. Embedding
is a **pluggable `embed(texts) → vectors` step, local by default** — three concrete
reasons over an API: the corpus may hold confidential content (an API means data egress on
every section), a rebuild-in-seconds/offline tool cannot depend on the network, and
re-embedding happens on every reindex. A **content-hash cache** in `.vaire/` makes that
cheap: reindex re-embeds only changed sections (git diff → changed files → changed
sections); a cold rebuild re-embeds once. Without the cache, "rebuildable in seconds"
breaks the moment embeddings exist. Providers are opt-in beyond the local default: a
`command` provider (shell out to any local model) and an `openai` provider (the OpenAI API,
accepting the egress) — the latter reads `OPENAI_API_KEY` from the environment or the
gitignored `.vaire/.env` (cli.md §6.2).

**On disk.** Everything Vairë owns lives under `.vaire/` in the repo root. That directory
holds two things with opposite lifecycles: one **committed** file — `config.toml`, the
authored, version-controlled settings (ID-prefix vocabulary, embedding config, include
globs) — and everything else, which is **derived and gitignored**: `index.db` plus its
WAL-mode `index.db-wal` / `index.db-shm` sidecars and lock, the embedding cache, and any
future derived artifacts. The gitignore rule encodes exactly that boundary — ignore all of
`.vaire/`, then re-include the one authored file:

```gitignore
# Vairë — derived index, rebuildable from files
.vaire/*
!.vaire/config.toml
```

So *everything in `.vaire/` except `config.toml` is the disposable derived layer.* Keeping
config beside the index it configures (rather than elsewhere in the repo) keeps all of
Vairë's footprint in one directory, while the gitignore whitelist preserves the rule that
nothing derived is ever committed.

The committed `.vaire/config.toml` is also what **marks a corpus**: `vaire` discovers the
root by walking up to the nearest `.vaire/` (cli.md §2.1), and creates `index.db` inside
that existing directory on first run. A fresh clone therefore has the truth (files) and the
config, and no index until it is built — which is correct, and reinforces
files-authoritative. Because the db is gitignored and per-checkout, every machine and agent
runs its **own** local Vairë over the same shared corpus files: there is no index to sync,
because each rebuilds from the canonical source.

**Indexing prefers commit (commit-as-publish), with a working-tree fallback.** When the
corpus root is a Git repo with commits, indexing reads the **committed** tree: committing
is the moment a record becomes real and findable (uncommitted = the author thinking out
loud; committed = published), and every index state corresponds to exactly one commit
(deterministic, consistent with Git provenance). When the corpus is **not** a Git repo, or
has no commits yet (a fresh corpus, or one nested inside a larger repo), there is no commit
to bind to, so indexing falls back to a full pass over the **working tree** on disk — the
recorded commit is then `null`. Commit-as-publish remains the intended workflow for a real
Git corpus (e.g. a `post-commit` hook), but `vaire` never *refuses* to index just because
nothing is committed.

**Two retrieval paths against the same files.** Authors open files and follow wikilinks
directly for focused/deep work — native fluency, no tool needed. The index serves only the
queries files are bad at. It *augments* file-native access, it does not replace it: **the
index points (returns paths + IDs); files hold (the caller opens for depth).**

**Interface — one CLI, MCP over STDIO.** Vairë is a CLI. `vaire mcp` starts a STDIO MCP
server that exposes the read commands as MCP tools — so there is no second implementation
to drift: **the MCP tools *are* the CLI commands.** Humans and scripts call `vaire …`
directly; agents spawn `vaire mcp`. Every read command takes `--json` (the same structured
shape MCP returns) and emits **paths + IDs**, so the caller opens the actual file for
depth. The full command surface, flags, output shapes, and exit codes are specified in
[cli.md](cli.md); the summary:

- **Read** (also the MCP tool surface): `resolve`, `render`, `backlinks`, `refs`,
  `search`, `suggest`, `unresolved`. (`render` is the one that returns a body — resolved
  Markdown — rather than pointers; `suggest` ranks existing IDs for a descriptor.)
- **Maintain** (run by humans / git hooks / CI, **not** exposed over MCP): `init`,
  `index`, `check`, `status`.

`resolve` / `backlinks` / `refs` / `search` are the one retrieval surface, used from both
**authoring** (lookup-before-reference, §7) and **retrieval** (reading the corpus to
work). `unresolved` feeds the §8 creation pass; `check` runs the validations that make
ID-based discovery safe. The command is `resolve`, not `resolve_entity` — Vairë operates on
*nodes by ID*, never "entities," consistent with being type-agnostic. Maintenance stays
off the MCP surface so the agent-facing toolset is bounded to reads.

## 10. Decisions locked (with rationale)

Kept with their reasons, because decisions without reasons get undone.

- **Files authoritative, index derived.** A derived index cannot corrupt truth; an
  authoritative DB would be a backup/protection liability. Agents are natively fluent in
  Markdown. Git gives provenance for free.
- **Git commit log is the provenance layer.** Who changed a fact, when, why, and what
  record in the same commit caused it — native, not built.
- **Entities global, records project-scoped.** A person referenced from three projects is
  one entity, not three.
- **References = IDs, never display names.** Names change, IDs do not — this is the exact
  thing that breaks otherwise.
- **Display name from `name:`, overridable inline.** A bare reference `[[type:id]]` renders
  to `[name](path)`, taking its text from the target's `name:`; `[[type:id|X]]` overrides
  with `X`. Renaming an entity updates the display of every reference to it for free,
  because records store the ID, not the name.
- **ID = `type:id`, composed from two frontmatter fields.** `id:` is the bare locally-unique
  slug; `type:` is the authoritative type *and* the ID namespace. They compose to the
  address (`id: hr` + `type: department` ⇒ `department:hr`). No redundant prefix-in-`id:` to
  keep in sync — `type:` is the single source of a node's type.
- **Optional project-scoped IDs for records** — a path of typed IDs,
  `<project-id>/<type>:<local>` (config `scoped_types`, cli.md §6.1). A scoped record writes
  only a container-local `id:`; the scope *is* its `scope:` value (the field is
  `scope_field`, default `scope`; the value names any container type), so composition is
  purely local (no per-container declaration,
  no cross-file lookup, single-pass), and uniqueness + stability come free from the
  container's ID. The node's own `type:id` is the last path segment.
  Removes the global-slug prefix tax (per-project `nova-…` naming) while keeping IDs
  globally unique and self-describing. Off by default (flat IDs).
- **Discovery by frontmatter, not path.** A node is any `.md` with an `id:` and a `type:`;
  layout is advisory. Vairë is a generic frontmatter-graph indexer, agnostic even to
  entity-vs-record. Forces two integrity checks at index time: ID uniqueness and reference
  resolvability. Trade-off accepted: `project:` and the `type:` field become the
  authoritative sources of scope and type (no longer inferable from path).
- **Hybrid search; embeddings local + cached.** FTS5 + the `aliases:` list carry precision
  (and most of resolution); vectors are the recall layer, primary only for open retrieval.
  Vectors live in the same SQLite db (no separate store). Embedding is pluggable and local
  by default (data egress, offline, re-embed-on-reindex), with a content-hash cache so
  rebuilds stay cheap.
- **Typed IDs — `type:id`** (`person:`, `department:`, `method:`, `system:`, `event:`,
  `record:`). The `type:` field is the ID namespace, so the index knows type without
  parsing prose; namespaces prevent slug collisions across types. Vocabulary will grow.
- **Frontmatter + inline wikilinks.** Frontmatter carries the structured edge list parsed
  without reading prose; inline preserves position and meaning.
- **Commit-as-publish.** Committing is when a record becomes real and findable;
  scratch-vs-durable falls out of Git semantics with no separate state machine.
- **Two retrieval paths.** File-native for depth; index for backlinks, traversal,
  staleness, semantic search. Index augments, not replaces.
- **Autonomous on records, gated on entities.** Records are additive and locally contained
  (safe to write autonomously); entity creation is the only irreversible, propagating act,
  so it is the only thing gated.
- **Unresolved references carry descriptors, never proposed IDs.** A provisional slug
  collides; a descriptor does not. Identity is created once, in the gated pass.
- **Resolution is additive.** Adds an ID target, preserves original phrasing as display
  text; records stay additive-only.
- **Supersession via frontmatter redirect.** `superseded_by: <id>`; the index follows it;
  old references still resolve.

## 11. Open questions

- **Entity-creation auto-vs-surface threshold.** Where the pass stops auto-creating and
  asks a human. Tune against the first week of real `[[?...]]` references, not in the
  abstract.
- **The entity-write step.** How a cluster of descriptors becomes an entity file:
  canonical-name selection, alias capture, type confirmation. Bounded; partly deferred to
  real data.
- **Merge UX.** Beyond the minimal `superseded_by` redirect — how a human or agent
  actually adjudicates and executes a merge when duplicates are found.
- **Outcome-grounded promotion (memory).** Memory consolidation typically promotes on
  retrieval-signal proxies for usefulness, not confirmation that the task a memory came
  from actually *succeeded*. If memory quality should track task success, that is custom
  work in the agent runtime, outside this spec. Noted only because it shapes how much to
  trust memory signals at the read-time boundary (§3.3).

---

## Build order

1. **Restructure the corpus**: entities with IDs + records referencing them. Fixes the
   original problem on its own, before any tooling.
2. **Build Vairë**: Rust CLI over `.vaire/index.db`, commit-bound via a git hook (`vaire
   index`). Ship the read commands — `resolve`, `backlinks`, `refs`, `search` — and `vaire
   mcp` to expose them over STDIO. This is the retrieval surface, which also powers
   lookup-before-reference authoring.
3. **Adopt the authoring contract** (§7) across all authors, human and agent, including the
   `[[?type: descriptor]]` convention.
4. **Add `unresolved` + the entity-creation pass** (§8). Let agents author with `?`
   references for a week first, then set the auto-vs-surface threshold from real loose
   ends.
