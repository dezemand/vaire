# Vairë — distribution: catalog, store, lockfile, registries

Status: **implemented in 0.3.0** except where §12 says otherwise.

[design.md](design.md) defines the corpus and its index; [manifest.md](manifest.md)
defines package identity and the `^MAJOR` constraint; [cli.md](cli.md) defines the exact
command surface. This document defines the layer between them: how a package is
**released**, where a released copy **lives on a machine**, how a dependency is
**located**, and what a **remote registry** has to serve.

Git remains the source of truth throughout. Everything here is a derived distribution
layer, and every part of it is reconstructible from a corpus and a registry.

## 1. Principles

> **The user never types a version number** — except to declare a major dependency
> (`vaire add name@^2`) or to deliberately cut a major (`vaire release --major`).

- **Versions are publication events, not editing state.** Between releases there is no
  version to manage. Knowledge accretes as commits; a version is cut when accumulation
  warrants, and the bump is *computed* from what changed (§3).
- **Local is a cache; the remote is the record.** A registry keeps every published
  version forever — a yank is a flag, never a deletion — which is what makes local
  pruning free, and lets retention and `vaire clean` be blunt (§5, §8).
- **A static file host is a full citizen.** The wire contract is a file layout first;
  server behavior is an optional capability. A directory, an S3 bucket, or any HTTP
  server that can serve five paths implements the whole protocol (§9).
- **Reported, never guessed.** Two directories claiming one name, a bump the classifier
  cannot decide, a registry that timed out: surfaced, never resolved by tiebreak.
- **Network by consent.** Only `push`, `pull`, `yank`, and `upgrade` reach the network.
  **Resolution never fetches.** A dependency this machine cannot satisfy is reported with
  the `vaire pull` that would satisfy it, and then somebody decides — because a tool that
  downloaded a package merely because a manifest mentioned one would make "what is my
  corpus?" unanswerable without a network trace.
- **Reads never mutate a checkout.** A read command may write to user-global caches under
  `~/.vaire`; it never writes to any package.

## 2. Terminology

| term | meaning |
|---|---|
| **workspace** | a live checkout of a package — a directory with `knowledge.toml`. What you author. |
| **release** | an immutable `(name, version)` publication: a git tag, a release record, a packed artifact. |
| **artifact** | the packed release: manifest, corpus files, everything they reference, and a prebuilt index. |
| **catalog** | `~/.vaire/catalog.db` — what this machine knows: workspaces it has seen, registries it is configured against, releases it holds. |
| **sighting** | a catalog row recording that a package declaring name *N* at version *V* was observed at path *P*. An observation, keyed by path. |
| **store** | `~/.vaire/store/<name>/<version>/` — pulled releases, unpacked and sealed. |
| **registry** | a remote source of published releases. May be a static file host or an API server; the client tells them apart only by declared capabilities. |
| **pin** | a consumer-side hold on an exact version, recorded in the lockfile. |

`~/.vaire` is the **vaire home**, overridable with `VAIRE_HOME`.

## 3. Release

### 3.1 The bump is computed

At release time the classifier diffs the entity index of the **last released artifact**
against the current tree. A semantic model to diff is what makes this mechanical rather
than a judgment call:

| observed in the diff | bump |
|---|---|
| entity IDs added; none removed or renamed | **MINOR** |
| section content changed; entity ID set identical | **PATCH** |
| entity IDs removed or renamed, or `superseded_by:` appeared | **MAJOR** — gated |
| mixed | highest applicable |

MAJOR is never taken automatically: the classifier refuses without an explicit `--major`.
The inverse also holds — `--major` may always escalate a textually small edit, because a
truth reversal is a semantic act and the maintainer owns meaning while the tool owns
structure.

The classifier compares sections, edges and alias text only. A touched `updated:` field
or a moved file is therefore not a release.

### 3.2 Release records

Every release writes an **entity describing itself** (conventional type `release`,
overridable with `release_type`): the version, the date, the computed bump, and edges to
the entities involved — `added`, `changed`, `retired`.

That the changelog is *corpus* rather than a file is what makes three things fall out
instead of being built:

- "Which releases touched this entity?" is an ordinary `vaire backlinks` query.
- The wire's changelog documents (§9.1) derive from the record, so there is one source.
- A consumer advancing a dependency can compute what changed **that it actually cites**,
  by intersecting these edges with its own (§6.4).

`removed` is recorded as text rather than references, and the asymmetry is not an
oversight: a removed entity has no address left to point at. It costs nothing, because
removals only happen in a MAJOR, where a human reads the notes anyway.

A **first** release writes a record like any other. Having no baseline to diff is not the
same as having nothing to say: the edges answer "which release published this?", and nothing
ever re-adds an entity that was already there, so a founding entity with no record would be
permanently unattributed.

A record's edges are **history, and may cite an entity a later release removed**. That is not
a dangling reference to be fixed — it cannot be corrected without lying about what was
published, and nobody typed it. It is exempt from the resolution lints accordingly; left as
an ordinary dangling reference it would fail `check` forever, and since `release` gates on
`check`, the package could never be released again.

The classifier **excludes the release type from its own diff**. Otherwise every release
would add an entity and no later release could ever classify as PATCH.

### 3.3 Release is versioning; push is transport

`vaire release` is a git act: classify, gate, write the manifest version, write the
record, commit, tag. It never pushes.

`vaire push` uploads whatever releases a registry lacks. Idempotent, retryable, and dumb
— a flaky upload re-runs nothing but the upload, artifacts are rebuilt from their own
tags so it works from a fresh clone, and **CI can publish a tag it did not cut**. The
race (two maintainers cutting the same version) resolves at the registry: the duplicate
`(name, version)` is rejected, the loser refreshes, recomputes the bump against the new
latest, and retries. Versioning is linear per package; there is no merge.

## 4. The catalog

`~/.vaire/catalog.db` records what this machine knows. It is an **inventory and a
mediator, never an authority**: the corpus is still the truth, every package's index
still lives beside it, and nothing here holds entity content.

### 4.1 An index, never truth

Every row is an observation, so the catalog is always rebuildable and never believed on
its own. Before anything is resolved from a sighting, the manifest at that path is
re-read and must still say the same thing — and **what that read finds is written back**.
A version bumped by a release is adopted; a renamed package's row follows it; a path that
no longer answers is marked missing. That write-back is the self-healing, and it is why
losing this file costs a rescan and nothing else.

### 4.2 Two states, no clocks

A sighting is `live` or `missing`. Nothing expires on a timer and nothing is removed
behind the user's back. Checking costs one `lstat`, so freshness is *observed* rather
than assumed to decay: a path that goes away is marked missing and excluded from
resolution, and the same path reappearing — a remounted drive, a restored checkout —
flips straight back to live. Removal is always explicit (`vaire catalog rm`).

### 4.3 Registration

| origin | how | precedence |
|---|---|---|
| `registered` | `vaire catalog add <path>` | outranks the others |
| `scanned` | `vaire catalog scan <dir>` | — |
| `ambient` | a maintain command touched the package | — |

Sightings are keyed by the **canonicalized** path, so two routes to one directory cannot
register twice and raise a false ambiguity. `--no-register` skips, and never forgets: an
existing sighting is left exactly as it was.

Store entries are never sightings. They are not working copies, and recording them as
such makes one directory arrive under two identities.

### 4.4 Concurrency

This is the first Vairë state used by more than one process at a time. The embedded
engine takes an **exclusive lock when a database is opened** — a second process cannot
open the catalog at all, not even to read it, while a first one holds it.

That is workable, because the lock *is* the mutex we would otherwise have had to build.
Two rules follow, and both are load-bearing:

- **Connections are short-lived and lazily opened.** Open, do the one thing, drop. A
  handle held across anything slow — a corpus walk, an index build, an HTTP request —
  locks every other vaire process on the machine out for that whole time. A command that
  turns out to have nothing to ask must not open it at all.
- **Contention is retried, never failed.** The holder is milliseconds from finishing, and
  an OS lock dies with its process, so there is no such thing as a stale catalog lock.
  Distinguishing *locked* from *corrupt* is essential: a version that treated every
  failed connect as corruption would delete the catalog whenever another process held it.

Writes are idempotent upserts regardless, so the worst case of a race is one observation
recorded twice.

### 4.5 Schema

The catalog stamps its own schema version. A version the binary knows how to bring forward
is **migrated in place**. Each step is idempotent, because nothing transacts the schema
change together with the version stamp — a process killed between the two has to be able to
finish on the next open.

Recreating was free while every row was an observation a rescan reproduces. **It stopped
being free when `vaire clean` began reading its roots from these rows** (§8): a pin, and the
record that a package was pulled by name, are not re-observable by anything, so forgetting
them means the next sweep deletes the releases they were holding. Two rules follow:

- **Unreadable must be proven, never inferred.** A failed connection says the engine did not
  get a database; it does not say the bytes are at fault. The file is re-opened directly, and
  only one this process can itself read and write is treated as garbage. A catalog that is
  merely unreachable — permissions, a half-mounted home — is an **error**, and an error
  deletes nothing.
- **A newer catalog is refused, not rebuilt.** The lockfile's rule (§7) applied to the same
  problem: refusing to *read* a format you do not know is only coherent if you also refuse to
  *overwrite* it.

What is displaced is kept beside the catalog rather than removed, so a wrong guess here costs
a file to look at rather than the record of what this machine holds.

## 5. The store

`~/.vaire/store/<name>/<version>/` holds what `vaire pull` fetched: the package's own
files exactly as its release shipped them, plus an index **this machine built**. A store
entry is a package directory like any other — the resolver links to one exactly as it
links to a working copy, and every read command above it neither knows nor cares which it
got.

### 5.1 The shipped index is a claim, never truth

An artifact carries a prebuilt index, and materialization **throws it away and rebuilds
from the shipped Markdown**. This is the load-bearing decision of the whole layer. A
published index is a file a publisher produced; adopting it would make every consumer's
answers depend on a stranger's build, and a corpus whose index disagreed with its own text
would have no way to be caught. Rebuilding costs about a second per package and removes an
entire class of trust question.

What *is* adopted is provenance — the release's commit, and how it was indexed — because
those are facts about the release rather than claims about the graph.

### 5.2 Materialization

Verify against the digest → unpack with containment → rebuild the index → record the
entry's own `source.toml` → seal → atomic rename into place.

Unpacking is the one place this codebase treats input as hostile. Entries that are
absolute, that climb out with `..`, or that are links of any kind are refused outright,
and under the index directory only the database itself is accepted. A refusal aborts the
whole materialization: a partially-unpacked artifact is never something to reason about.

`source.toml` makes the entry self-describing — name, version, the artifact digest it came
from, the registry it came from, and which vaire built it. The catalog's release rows are
an index over these files, so losing them costs a walk of the store.

### 5.3 Sealed, but not entirely

The corpus files and `source.toml` are made read-only. That is what lets "answered against
acme-core 1.4.2" be a claim anyone can check.

The index directory is **not** sealed, and the reason is a property of the engine rather
than a compromise: a database is opened read-write even to read it, so a sealed index is
an *unreadable* one. Immutability is enforced by never rebuilding the entry; the
permissions are a backstop against accidental edits, not the mechanism.

Sealing is applied to the contents, then the directory is renamed, then the root itself is
sealed — a rename rewrites the parent entry of a directory, so a read-only directory
cannot be moved into place.

### 5.4 Retention

**One slot per major line, plus pins.**

- On pull, 1.4.2 *replaces* 1.4.1. This is safe by the protocol's own contract: the
  meaning-change predicate guarantees within-major substitutability, so the rule is
  derived from an invariant rather than bolted on.
- Two majors coexist exactly when the transitive `^N` demands differ. Automatic, and
  trivial to compute because constraints are majors-only: there is no solver.
- A pin survives replacement (§6.3).

Removal is never fatal. A version that could not be removed is a warning, because the
pull it follows has already succeeded and the only cost of a stale sibling is disk.

## 6. Resolution

### 6.1 Order

```text
explicit link (.vaire/packages/<name>)   per-consumer override, authoring
→ run-root self-reference
→ the catalog          a working copy, selected by the ^MAJOR constraint
→ the store            a pulled release
```

First hit wins, and **nothing here reaches the network**.

### 6.2 The two-worlds rule

A working copy outranks a pulled release of the same name. A checkout is what you are
authoring, and resolving to a published copy of it would quietly answer against yesterday.

The corollary is that only a store-resolved answer is reproducible, which is what
`knowledge.lock` records (§7) and `--frozen` enforces.

### 6.3 Selection and ambiguity

The `^MAJOR` constraint is a **selector**, not merely a lint: the catalog picks the package
whose declared version satisfies it, comparing parsed triples rather than text. Where
several members of one closure constrain the same name, their demands are intersected —
one major line resolves; disjoint majors are a reported conflict, because a closure links
one directory per package name.

What survives the constraint filter is **refused, not tiebroken**. Two remaining candidates
are two working copies of the same major — a fork beside its original, two worktrees on
different branches — and picking the higher version would be a guess dressed as
arithmetic, since a fork is routinely newer than what it forked from. One tier applies
first: an explicit `vaire catalog add` outranks a path something noticed in passing,
because that is a statement of intent, and it gives ambiguity a resolution that is not
"edit every consumer's links".

A **pin** selects within the store: the pinned version is taken instead of the highest
satisfying one. It does not change *which world answers* — an explicit link and a working
copy still outrank the store. The lockfile is committed, so a pin that displaced checkouts
would reach every colleague's authoring setup.

Closure conflicts are judged **after** the links settle. Judged mid-walk, a conflict would
be invisible whenever the first constraint seen happened to resolve, and the outcome would
depend on link order.

### 6.4 The adopted-changes digest

Advancing a dependency raises one question — what changed under me? — and the publisher's
changelog is almost never the answer, because it describes everything that happened to a
package most of whose entities a given consumer has never cited.

Both halves of the better answer are already in the graph: a release record carries edges
to the entities it touched (§3.2), and the consuming package's index carries edges to what
it references. The digest is their **intersection**, reported when a pull replaces a
version. Short by construction, and specific to this consumer.

Nothing about it can fail a pull. The bytes arrived and are sound, so a digest that could
not be computed is a missing courtesy, not a failed acquisition.

## 7. `knowledge.lock`

Written by `pull` and by the ensure pass, never by hand. It records the **whole closure** —
reproducing a resolution means reproducing all of it — and each entry says how it resolved:

| `source` | records | reproducible |
|---|---|---|
| `registry` | version, registry, artifact `sha256` | yes — `pull --locked` fetches exactly those bytes |
| `workspace` | version | no, and the missing digest says so |

The split is the two-worlds rule written down rather than papered over. A checkout has no
artifact to checksum and can change between two runs; writing a digest for it would be a
reproducibility claim the tool cannot keep.

A stale lock is safe and merely imprecise, since within-major substitutability is the
protocol's own promise. That is what makes it reasonable to commit in leaf packages —
where the citability claim lives — and to treat it as informational elsewhere.

Rules that each exist because the obvious alternative is silently wrong:

- **`--locked` verifies against the recorded digest**, not the published one. `fetch`
  already confirms an artifact matches the registry's *current* claim, so the lockfile is
  the only thing that can catch a registry serving different bytes under a version it has
  already published.
- **An entry with no checksum is refused, not skipped.** Passing over one would let a
  pipeline report a reproduction it did not perform.
- **A refresh merges rather than replaces.** A run that could not reach a dependency keeps
  its previous entry, because that record is what somebody else reproduces from. Only a
  name the manifest no longer declares is forgotten.
- **A refresh never moves a pin.** Carrying the flag onto a newly resolved version would
  keep the hold in name while releasing what it held.
- **A lockfile from a newer vaire is refused, not reinterpreted** — and refusing to read
  one means refusing to *overwrite* it, and refusing to sweep on the assumption that it
  holds nothing. A refusal is only coherent if every consumer of the file honors it the
  same way.
- **An empty lockfile is removed rather than written.**

`--frozen` is the global flag that turns the record into enforcement: resolution answers
only from the store, and a dependency resolving to a working copy is refused with the
`vaire pull` that would fix it. The gate sits *after* resolution, so there is one
resolution order to reason about and the refusal can name what it found. It never consults
the catalog — that is the index of working copies, exactly what this mode refuses, and
skipping it also keeps CI off the machine-wide catalog lock.

## 8. `vaire clean`

The store is disposable, so a sweep can be blunt; what it must not be is surprising. The
rule is therefore stated as what is **kept**:

- **Locked** — a version some registered workspace's lockfile names.
- **Pinned** — a version some workspace pinned.
- **Requested** — a package pulled *by name from outside any package*. That pull writes no
  lockfile, deliberately, because there is no resolution to record — and it is exactly how
  a reader with no package of their own assembles a corpus. Rooting only what a lockfile
  names would delete their whole library. A named pull *inside* a package is not a standing
  request: the lockfile records it instead. `vaire clean <name>` withdraws one.

Everything else is a leftover: a version retention could not remove, a major line no
manifest declares any more, a transitive dependency that left every closure.

Roots come from the catalog, so a package this machine has never recorded contributes none
— its lockfile is a file nobody knows to read. That is the third time registration pays for
itself, and it is why `vaire pin` records the package it runs in.

## 9. The wire contract

Everything a static host can serve, with **per-package index documents** rather than one
global document:

```text
/.well-known/vaire-registry.json          descriptor: schema version, name, capabilities
/v1/packages.json                         [names] — every package writes it, so it merges
/v1/index/<name>.json                     the package's release index
/v1/artifacts/<name>/<name>-<ver>.tgz     the packed artifact, immutable once written
/v1/changelogs/<name>/<ver>.md            readable *before* pulling a major
```

### 9.1 The documents

The **descriptor** declares `schema_version`, the registry's name, and its capabilities:
`search`, `enumerable`, `publish`, `yank`, `validate_bump`, `access_enforcement`. The
schema version gates hard — an older client refuses a newer-major registry cleanly rather
than misreading it.

The **index document** carries, per release: `version`, `sha256`, `size`, `published_at`,
`yanked`, `deps`, `description`, `changelog_excerpt`, and a reserved `signatures` slot.
`deps` lives here so transitive resolution never downloads an artifact to read a manifest.
`changelog_excerpt` carries a MAJOR's invalidated-assumptions summary, so a dependent can
decide about re-confirmation before fetching anything.

### 9.2 Publishing

Immutability and race handling are properties of the **write**, which is what lets a dumb
file host enforce its own semantics with no server code:

1. `PUT` the artifact **create-only**. Storage itself rejects a duplicate `(name, version)`.
2. `PUT` the changelog document.
3. **Compare-and-swap** the index document. A CAS failure is the push race: refresh,
   recompute, retry.
4. **Read-merge-CAS** `/v1/packages.json`, adding this name if absent.

Step 4 is the only document *every* package writes, so it is the only one where two
publishers of **different** packages can collide — hence merge rather than overwrite, and a
compare-and-swap rather than a blind write. It is also the only step that is
**best-effort**: a name missing from the enumeration costs discovery, not resolution, since
an exact-name lookup reads `/v1/index/<name>.json` directly. A publish that succeeded in
every way that matters must not be reported as failed because this one document was busy.

**An interrupted publish can be finished.** Only the first write is atomic, so the window
between the three is real: a push killed after the artifact lands leaves the index silent
about it. Refusing on the next attempt would make that state permanent — the identity would
be immutably taken and no later push could claim it — which is the opposite of the
retryable, publish-a-tag-you-did-not-cut contract (§3.3). So the question is decided **by the
bytes**: an artifact on the host that is byte-identical to the one in hand, with no index
entry naming it, is this push's own earlier attempt, and the push completes what it finds
half-done. Different bytes under the same version stay a refusal.

A yank is an index edit; the artifact never moves. That is the whole difference between a
yank and a deletion — a yanked version is skipped for a *new* resolution and stays fetchable
by exact version, so anything already pinned to it keeps resolving.

Over `file://`, create-only is exact and compare-and-swap is best-effort (compare, then
rename) with a documented window. A lock file was considered and rejected: stale-lock
detection breaks locks it should not.

### 9.3 Access flags

Two orthogonal axes, both defaulting true when absent so every existing index document
stays valid: **listed** (appears in enumeration and search) and **pullable** (the artifact
may be fetched). Three states fall out — **open**, **restricted** (listed, not pullable),
**unlisted** (pullable by exact name, invisible to search). Entirely private is not a flag;
it is absence, or a separately-ACL'd registry.

Restricted-listed is a workflow, not a wall. Its purpose is preventing duplicate work by
routing people to the owner, so the `hint` is the feature and the refusal carries it
verbatim.

Access is a property of the **(package, registry) pair**, set at push and stored only in the
registry's index document — never in the manifest. The artifact stays access-agnostic, and
the same package may be open on a lab registry and restricted centrally.

**On a static host the flag is advisory**, and the client says so when you set one: anything
the bucket serves, bucket-readers can fetch. The real primitive there is the bucket
boundary. Where enforcement exists, restricted answers 403 — existence *should* leak, that
is the point — and truly private answers 404 everywhere.

## 10. The client

One trait, `Registry`, with `StaticHttp` implementing the contract above over a pluggable
transport. Splitting transport from protocol is what lets `file://` run the *real*
implementation, which in turn is why the test suite is a conformance suite rather than a
mock.

The trait is **synchronous**, against the earlier async sketch: there is no async runtime
above the storage engine's bridge, and an `async fn` in a trait is not dyn-compatible while
`Box<dyn Registry>` is exactly what the multi-registry fan-out needs.

### 10.1 Errors are load-bearing

A fan-out over several registries needs to know what a failure *means*, not merely that one
happened. Every error carries a disposition: continue to the next registry, remember it,
degrade, report partial results, or stop. Two distinctions matter most:

- `NotFound` moves on and is forgotten; `PullRestricted` moves on and is **remembered**, so
  if nothing else serves the package the user is told where to ask.
- An HTTP 403 is *unreachable*, never *not found*. Reporting a package as absent because
  permission was refused would send somebody looking for the wrong problem.

Registries are asked in priority order and the **first that satisfies wins** — not the
highest version across all of them. A registry is a trust boundary as much as a location, so
preferring a higher version from a lower-ranked one would let any registry on the list
outbid the one you meant to use.

### 10.2 Names are checked at the boundary

A package name becomes a path segment under the user's home and a path in a URL, so it is
validated where a declared name first becomes either — in the registry client *and* in the
store, since the store is consulted before any registry is asked. The boundary rule is:
starts with a lowercase letter or a digit, thereafter lowercase ASCII with `-`, `_` and `.`,
at most 128 characters.

**That is deliberately wider than the manifest's own grammar** (`[a-z][a-z0-9-]*`,
manifest.md §3), and the two are answering different questions. The manifest rule governs a
name you are *creating*; this one governs a name that arrived from a lockfile, a command
line, or a stranger's index document, where the only question is whether it is safe to make
a path out of. A containment check that rejected unfamiliar-but-harmless names would refuse
to fetch packages a future manifest grammar might well allow.

The looser rule costs nothing, because the manifest grammar is enforced again where it
matters. Materialization loads the unpacked `knowledge.toml`, and an invalid `name` fails
that load — so an artifact declaring a name this tool would refuse to author is refused on
the way into the store, alongside the check that its declared name matches the one it was
served under. A name that passes the boundary and fails the manifest costs a wasted
download; it never becomes a store entry.

## 11. Artifacts

The unit a registry stores and a consumer pulls: `<name>-<version>.tgz`, a gzipped tar with
one top-level directory holding the manifest, every corpus file the include/exclude globs
select, **every file those reference**, and an exported index.

**Inclusion is by reference, not by location.** A relative Markdown link or image pulls its
target into the artifact, transitively through referenced Markdown — so no directory is
reserved and nothing unreferenced ships. An orphan is not excluded by a rule; it simply has
no path into the archive. A link whose target is missing from the committed tree fails the
pack, because an artifact that is not self-contained is worse than one that was not built.

Everything is read **from the committed tree**: what you commit is what you publish. That is
what makes the artifact reproducible — sorted entries, timestamps pinned to the commit,
zeroed ownership, untimestamped compression, and **nothing recording which vaire packed it**,
since a builder's own version in the bytes would rehash a tag on every upgrade — and
reproducibility is what lets `push` rebuild
a release from its tag and get byte-identical output, so the checksum a lockfile pins belongs
to the release rather than to whoever uploaded it.

### 11.1 Embeddings do not travel

Artifacts ship **stripped**: no vectors. Embeddings are a consumer's choice of provider and
model (manifest.md §6), so shipping them would mean adopting a stranger's embedding space.
Materialization runs the consumer's own embedder.

No embedder configured is a **degradation, not a failure**: the entry is still built and
still searched lexically, reported as a warning. A package you can read is worth more than a
pull that refused.

The full-text index is likewise rebuilt rather than shipped, because its on-disk form
embeds identity that is not portable between builds.

## 12. Not built in 0.3.0

Named because the shape is decided and the absence is deliberate:

- **`push` over `http(s)`.** Reads work fully over HTTP; the create-only/CAS write path is
  wired for `file://` only. CI publishes by fetching, pushing into a local `file://` copy,
  and uploading — which uploads exactly one artifact however long the history is.
- **A search endpoint, and client authentication.** `Registry::search` is an unwired seam,
  and there is no auth anywhere. Both are 0.4 work, and they arrive together because both
  need a server that knows who is asking.
- **`serve` and the browse API.**
- **Signing.** The `signatures` slot exists so adding it later is not a schema break; no
  scheme is designed.
