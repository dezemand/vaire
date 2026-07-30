---
name: vaire-packages
description: >-
  Explains the Vairë knowledge-package model — what a package is (an ownership and consumption
  boundary with a declared identity), the `knowledge.toml` manifest, declaring dependencies with
  `^MAJOR` constraints, linking where dependencies live (`.vaire/packages/`, `vaire add --link`,
  the configured local-packages root), how cross-package `@pkg/type:id` references resolve,
  versioning semantics (MAJOR/MINOR/PATCH, tombstones, re-confirmation), and the integrity
  checks that guard the dependency surface. Use this when working with manifests or dependencies
  in any way — creating or editing `knowledge.toml`, adding or linking a dependency,
  understanding why an `@pkg/…` reference does or doesn't resolve, setting up a cloned package,
  interpreting `vaire deps` output, or reasoning about what a version constraint or bump means.
metadata:
  project: vaire
---

# Vairë packages

A **package** is the unit of ownership and consumption: a directory tree with a committed
`knowledge.toml` at its root, containing entity and record files (see the **vaire-files**
skill). Packages exist for two reasons — **autonomy** (this knowledge moves, versions, and
is owned on its own cadence) and **selective consumption** (someone wants *this* knowledge
without the rest). A package is a published boundary, not a folder; for the judgment of
*when* something deserves to be one, see the **vaire-package-curation** skill.

**Identity is declared, never derived from the directory.** The manifest's `name =` is the
package id, so a package can be relocated, vendored, or fetched into a cache without
rewriting a single reference.

## The manifest — `knowledge.toml`

The one authored config file, committed at the package root (it is also the package
marker — discovery walks up to the nearest one). Everything under `.vaire/` is derived and
gitignored.

```toml
name    = "acme-web"                    # identity: a slug, declared, not path-derived
version = "1.4.2"                       # semver MAJOR.MINOR.PATCH
description = "The web application knowledge package"   # one line, optional

include = ["knowledge/**/*.md", "projects/**/*.md"]     # file scope (these are defaults)
exclude = ["**/node_modules/**", "**/drafts/**", "**/archive/**"]

types = ["service", "component"]        # the entity types this package DEFINES
vocabulary_strict = false               # when true, `vaire check` warns on undeclared types

[dependencies]
acme-core = "^1"    # service → team, person edges
acme-ui   = "^2"    # component → pattern edges
```

- Only `name` and `version` are required; a two-line manifest is a valid package.
- `types` declares the vocabulary this package **defines** (the `type:` field / ID
  namespace). Omitted means it defines none. Two packages may declare the same type name —
  that never collides, because cross-package references are always `@`-qualified.
- `drafts/**` and `archive/**` are excluded by default: work parked there is invisible to
  the index and to consumers — the sanctioned place for incubating material.
- The comment on each dependency naming the edges that need it is a convention worth
  keeping: it makes `[dependencies]` self-explaining.
- Machine and consumer settings (notably embeddings) are **not** manifest keys — how a
  corpus is indexed is a consumer choice, set via `vaire configure`. Never add an
  `[embeddings]` table.

## Dependencies: declare, then link

Depending on another package is two separate facts:

1. **What** you depend on — declared in the committed manifest: `acme-core = "^1"`.
2. **Where** it lives on this machine — per-checkout state, never committed.

**`^MAJOR` is the only legal constraint form** (`^1`, `^2`, …). Minor and patch changes
never break references by definition, so tighter pins would only create churn; a dependent
rides a major line and adopts its minor/patch updates automatically.

Declare with `vaire add <name>[@^N]` (default `^1`) — it edits `[dependencies]` in place,
preserving formatting and comments. With `--link <path>` it also wires up where the
dependency lives:

```bash
vaire add acme-core --link ~/Knowledge/acme-core   # declare + link in one step
```

### Linked packages — `.vaire/packages/<name>`

Each link is a symlink (or directory) whose target is a package directory whose manifest
**declares that same `name`** — a name mismatch is an error, not a guess. Links are
gitignored, per-checkout state; the committed manifest carries only `name = "^MAJOR"`.

An `@pkg/type:id` reference resolves through the *referencing* package's dependencies: the
package alias must be in its `[dependencies]` (else `undeclared_import`), then the target
package is located at, in order:

1. the referencing package's own `.vaire/packages/<name>`, else
2. the **run-root package itself**, when the name is the run-root's declared name (a
   dependency cycle back into the package you're standing in needs no link), else
3. the run-root's `.vaire/packages/<name>` — the fallback that lets one flat set of links
   at your top level serve the whole transitive closure.

Own links win; the fallback is a convenience. Each linked package keeps **its own index**
inside its own `.vaire/` (federated — never a merged database); `vaire index` refreshes
dependency indexes through the links, and read commands never build.

### The local-packages root — zero-wiring clones

Tell Vairë where your packages live, once, machine-wide:

```bash
vaire configure local-packages ~/Knowledge
```

With that set, a declared dependency with no link entry is **satisfied automatically**:
the root is searched for a package *declaring* that name and the link is materialized. So
a fresh clone needs no wiring step — `git clone … && cd acme-web && vaire index` links
every declared dependency it can find, then builds. The rules:

- **Matching is by declared name, at any depth** — directory names are irrelevant.
- **Ambiguity is reported, never guessed.** Two packages under the root declaring one name
  leave the dependency unsatisfied, with both paths named — link the one you want
  explicitly.
- **Only gaps are filled.** An existing resolvable entry is never rewritten (explicit
  `--link` always wins); a *broken* entry is re-discovered, healing a moved package.
- Only commands that already write links discover: `vaire add`, and the ensure pass of
  `vaire index` / `vaire check`. A read command never materializes a link.

Inspect the result with `vaire deps` — the resolved dependency tree, from live link
inspection (no index needed, safe as a first command anywhere).

## Cross-package references

Grammar recap (the full reference model is in the **vaire-files** skill):

```text
[[@acme-core/team:platform]]            inline (prose)
owner: "@acme-core/team:platform"       frontmatter — quoted; @ is YAML-reserved
```

- Every `@pkg/…` reference must name a **declared** dependency. Declare it in the same
  change that introduces the reference.
- **Loose ends stay package-agnostic**: `[[?team: the platform owners]]` never carries a
  package — a descriptor is unresolved by definition, so its package is unknown. Never
  guess a package any more than you'd guess an ID; resolution assigns both.
- All read commands accept `@pkg/type:id` and follow `superseded_by:` tombstones across
  package boundaries. Fan-out reads (`search`, `backlinks`, …) list unavailable
  dependencies under `skipped` rather than silently dropping them.

## Versioning semantics

| bump | meaning | effect on dependents |
|---|---|---|
| **MAJOR** | an entity was removed/renamed/merged, or a confirmed claim changed such that a dependent's assumptions may now be wrong | flagged for re-confirmation |
| **MINOR** | new entities/knowledge within the boundary | silent (auto-adopted) |
| **PATCH** | clarification/correction, no meaning change | silent (auto-adopted) |

**MAJOR does not mean broken links.** A major that renames or merges an entity leaves a
tombstone (`superseded_by:`), and `resolve` follows it — so references keep resolving, and
MAJOR means *"re-confirm your assumptions"*, not *"your links are dead"*. Only deleting
without a tombstone truly breaks a reference, and `vaire check` treats that as an error
(`dangling_ref`). Deciding *which* bump a change is, and the release flow, is the
**vaire-versioning** skill.

## What guards this surface

`vaire check` enforces the dependency contract; the findings and their sanctioned fixes
are in the **vaire-check-triage** skill. The dependency-specific ones:

| finding | severity | when |
|---|---|---|
| `undeclared_import` | error | an `@pkg/…` reference whose package is not in `[dependencies]` |
| `dangling_ref` (cross-package) | error | a declared, linked reference whose target — after tombstone-following — does not exist |
| `missing_dependency` | error | a declared dependency that is unavailable (not linked / broken link / name mismatch); reported once per name, with the fix |
| `unused_dependency` | warning | declared but never referenced |
| `dependency_version_mismatch` | warning | a linked dependency's MAJOR outside the `^N` constraint (surfaced only; enforcement is future work) |
