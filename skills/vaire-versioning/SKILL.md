---
name: vaire-versioning
description: >-
  How to version and release a Vairë knowledge package. Use this when cutting a release,
  deciding whether a change is MAJOR, MINOR, or PATCH (the meaning-change predicate), renaming,
  merging, or removing entities (the tombstone obligation), writing the changelog, tagging, or
  responding as a consumer to a dependency's MAJOR bump (re-confirmation). Maintainer work:
  versions are a package's published promises, so only its maintainer bumps them.
metadata:
  project: vaire
---

# Versioning a Vairë package

A version is a **statement about meaning**, not about diff size. Dependents declare
`^MAJOR` and adopt minor/patch updates automatically (mechanics in the **vaire-packages**
skill), so the entire protocol rests on the maintainer bumping the right component.

## The meaning-change predicate

Ask one question per change: **could a dependent's assumptions now be wrong?**

| bump | when | dependents see |
|---|---|---|
| **MAJOR** | an entity was removed, renamed, or merged; or a confirmed claim changed such that a dependent's assumptions may now be wrong | a re-confirmation flag |
| **MINOR** | new entities or knowledge within the boundary | nothing (silent adopt) |
| **PATCH** | clarification or correction with no meaning change | nothing (silent adopt) |

Boundary cases, resolved:

- **The README interface is the contract.** A package's "questions this package answers"
  list (see the **vaire-package-curation** skill) is its interface: a change that
  invalidates one of those answers is exactly what MAJOR means, even if no entity moved.
- **Correcting an error**: if the old claim was one a dependent may have *acted on*, the
  correction is MAJOR — "no meaning change" describes wording, not truth reversals.
- **Adding aliases, fixing typos, resolving loose ends**: PATCH (resolution adds
  information but changes no claim; new *entities* created by the pass make it MINOR).
- **Moving knowledge out to another package** (with tombstones): MAJOR for the source
  package — dependents must repoint, even though nothing dangles.

## MAJOR never dangles — the tombstone obligation

A MAJOR that renames, merges, or relocates an entity **leaves a tombstone**:
`superseded_by: <type:id>` (cross-package allowed) in the old node. `resolve` follows it,
so every existing reference keeps resolving; MAJOR means *"re-confirm your assumptions"*,
not *"your links are dead"*. Only deleting without a tombstone truly breaks references —
`vaire check` reports that as `dangling_ref`, and a release does not ship until check is
clean. Tombstones are permanent: an ID, once published, resolves forever.

## Cutting a release

1. **Survey what shipped**: `git log` since the last tag; classify each change with the
   predicate above. The bump is the **max** over all changes.
2. **Tombstone audit** (MAJOR only): every removed/renamed/merged entity has
   `superseded_by:`; nothing was deleted bare.
3. **Gate**: `vaire check --strict` clean — violations *and* warnings; a release is the
   one moment warnings must be zero (fixes per the **vaire-check-triage** skill).
4. **Bump** `version =` in `knowledge.toml`.
5. **Changelog**: a dated entry per version. MINOR entries say what knowledge arrived;
   MAJOR entries name each invalidated assumption and where its tombstone points — the
   changelog is what a dependent reads to re-confirm.
6. **Commit and tag** (`v<version>`). The tag is the citable release.

Batching is fine and normal: knowledge accretes as commits, versions are cut when the
accumulation warrants a statement. Drafts (`drafts/**`) are outside the index and never
force a bump.

## Consuming a dependency's bump

- **Minor/patch**: nothing to do — `^MAJOR` adopts them silently.
- **MAJOR**: re-confirmation work. Read the dependency's changelog; for each invalidated
  assumption, find your affected references (`vaire refs`/`backlinks`, or search for
  `@pkg/`), repoint tombstoned IDs to their successors, re-verify claims your package
  makes on top of theirs, then raise your own `^N` and re-run `vaire check`. Until then,
  `dependency_version_mismatch` warns — surfaced, not enforced.
- A MAJOR in a dependency that invalidates *your* published answers cascades: your
  re-confirmation may itself be a MAJOR by the same predicate.

## Don't

- Don't bump MAJOR "to be safe" — false alarms teach dependents to ignore the flag.
- Don't ship a MAJOR without tombstones, or delete a tombstone later.
- Don't use pre-releases or build metadata — `MAJOR.MINOR.PATCH` only; unpublished work
  is just uncommitted or drafts.
- Don't let contributors bump versions; the merge that includes a bump is a maintainer
  act.
