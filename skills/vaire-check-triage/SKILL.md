---
name: vaire-check-triage
description: How to interpret and fix every `vaire check` finding. Use this when `vaire check` (or `vaire check --strict`) fails or warns and the finding needs its sanctioned fix — duplicate_id, dangling_ref, undeclared_import, missing_dependency, drift, orphan, frontmatter_wikilink, unknown_type, unreferenceable_id, scoped_type_not_permitted, unused_dependency, or dependency_version_mismatch. Covers what each kind means, the fix that respects the corpus rules (tombstones not deletions, loose ends not guessed IDs), and which findings block publication.
metadata:
  project: vaire
---

# Triaging `vaire check`

```bash
vaire check [--strict] [--working-tree] [--no-deps] [--json]
```

`--working-tree` validates uncommitted edits (the edit → validate loop); `--strict`
promotes warnings to failures — required for a release, recommended before any merge.
Exit `0` clean, `6` on any violation (or any warning under `--strict`). With `--json`:
`{ "ok", "violations": [{kind, …}], "warnings": [{kind, …}] }`.

**A clean check is the definition of done for any change.** Fixes must respect the corpus
rules: repair by *addition* (tombstones, declarations, loose ends) — never by deleting
nodes or inventing IDs to silence a finding.

## Violations (always block, exit 6)

| kind | it means | the sanctioned fix |
|---|---|---|
| `duplicate_id` | two nodes share one `type:id` | keep the better node; the loser gets `superseded_by: <winner>` and stripped content — never delete either. If they are genuinely different subjects, one gets a *new* slug only if it was never published; a published ID is never recycled |
| `dangling_ref` | a non-`?` reference whose target is not a node (local, or cross-package after tombstone-following) | a typo → correct the ID (`vaire suggest` to find the intended target); the target truly doesn't exist → demote the reference to a loose end `[[?type: descriptor]]` — never mint the missing entity to fill the hole (that is the gated pass — **vaire-entity-creation** skill); the target was deleted → restore it as a tombstone |
| `undeclared_import` | an `@pkg/…` reference to a package not in `[dependencies]` | the reference is right → `vaire add <pkg>` (declare in the same change); wrong → fix or demote it. Never guess a package on a loose end |
| `missing_dependency` | a declared dependency is unavailable: not linked, broken link, or the link target declares a different name | follow the message's fix — `vaire add <name> --link <path>`, or configure the local-packages root and re-run `vaire index` (**vaire-packages** skill). A name mismatch means the link points at the wrong directory |

## Warnings (advisory; `--strict` promotes)

| kind | it means | the sanctioned fix |
|---|---|---|
| `frontmatter_wikilink` | `[[ ]]` brackets in a frontmatter value — silently *not* an edge | unbracket it: `owner: department:hr` (quoted if `@pkg/…`) |
| `unknown_type` | a frontmatter value shaped like a reference (`field: team:alpha`) whose type is not declared — it was ignored, not made an edge | intended as a reference → declare the type, but only deliberately, via the ladder in the **vaire-package-curation** skill; plain text that happens to contain a colon → quote it as a string |
| `drift` | a resolved inline reference whose target is missing from the frontmatter edge list | add the frontmatter edge if the relation is structural; narrative-only mentions may legitimately stay inline |
| `orphan` | a node with no inbound or outbound edges | usually under-linking — connect it from/to its neighbours; if nothing would ever reference it, it may fail the substance bar (fold it into its parent per the **vaire-entity-authoring** skill) |
| `unreferenceable_id` | the node's declared `id`/scope falls outside the reference grammar (`id: Jane_Doe`) — it indexes, but nothing can address it | fix the slug (lowercase, digits, hyphens) *before* anything references it; already-published IDs get a correctly-slugged successor plus a tombstone |
| `scoped_type_not_permitted` | a scoped node's type violates the manifest's whitelist/blacklist lint policy | either the scope is wrong (drop it) or the policy is out of date (a maintainer widens `scoped_types_whitelist`) |
| `unused_dependency` | declared in `[dependencies]`, never referenced | remove the entry — unless it is deliberate (e.g. declared ahead of imminent references); then keep it, commented |
| `dependency_version_mismatch` | a linked dependency's MAJOR is outside your `^N` | do the re-confirmation work, then raise your constraint (**vaire-versioning** skill); or you linked the wrong checkout |

## Order of attack

1. `missing_dependency` first — unavailable dependencies make their edges *skip* the
   dangling pass, so other findings may be hiding behind it.
2. Then the remaining violations, then warnings.
3. Re-run after each batch (`--working-tree` while uncommitted). Exit `3` (index corrupt)
   is not a finding — rebuild with `vaire index --full`; exit `4` means no index yet —
   run `vaire index`.
