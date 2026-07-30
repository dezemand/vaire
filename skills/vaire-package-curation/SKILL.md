---
name: vaire-package-curation
description: >-
  Maintainer judgment for Vairë package boundaries and ontologies. Use this when deciding
  whether knowledge deserves a new package (or should stay put), scaffolding one, writing or
  auditing a package README against the substance bar ("questions this package answers"),
  deciding whether to add a new entity type (the reuse → facet → new-type ladder), moving
  knowledge between packages with tombstones, or judging whether a past split should be merged
  back. Pairs with vaire-versioning for the release consequences of these decisions.
metadata:
  project: vaire
---

# Curating a Vairë package

A package is an **ownership and consumption boundary, not a folder** — granularity
follows ownership, never directory accident. Curation is the maintainer's judgment work:
what enters, what leaves, what the ontology admits, and what the README promises. The
package mechanics live in the **vaire-packages** skill.

## When to create a new package

Create one only when **all four** hold:

1. **It has an owner** who is not already the maintainer of the package it would
   otherwise live in — or the same owner genuinely versions it on a different cadence.
   No owner → it cannot be maintained → it will rot. Hold it as drafts instead.
2. **Someone would depend on it alone.** Name a real consumer who wants this subject
   without the surrounding material. If every plausible consumer would also pull the
   parent, the split only adds `@pkg/` noise.
3. **You can state its boundary in two sentences** — what it covers *and* at least one
   thing it deliberately does not. "Stuff about X, roughly" is not a package yet.
4. **It passes the substance bar on day one** — at least three real questions it
   answers, each backed by entities that exist at publication.

Two sanctioned exceptions to rule 4, both declared in the manifest/README:

- **The namespace placeholder** — an empty package that exists so another org's
  `department:`/`person:` entities never pollute a neighbour's namespace. Legitimate,
  but say so in its description.
- **The incubating package** — future-package material parked under an existing
  package's `drafts/**` (excluded from its index) until it earns existence; the parent's
  README names the destination so the parking is visibly temporary.

Signs you should **not** split: the candidate would never cut a version the parent
doesn't cut at the same moment · it is one entity plus its neighbours (that is a
*section*) · the motivation is tidiness (size is not a boundary; ownership is) · you
would be both its maintainer and its only consumer, forever. And the reverse check,
periodically: a package that has never versioned independently of its one consumer was a
premature split — merge it back, with tombstones.

Scaffold: `vaire init` in the new root, then fill in `name`/`version`/`description`/
`types`, declare dependencies with a comment naming the edges that need each one, and
write the README below.

## The substance bar — what the README must answer

A package is a *published answer to questions*, not a place files go. Its README (plus
manifest) must answer, without the reader opening one entity:

1. **What is this?** — one line (manifest `description`).
2. **What questions does it answer?** — at least three, phrased as a consumer would ask,
   each with a pointer to where the answer starts (`→ service:ingest-api`, …). This list
   is the package's **interface**; invalidating one of these answers is what MAJOR means
   (**vaire-versioning** skill).
3. **What is out of scope, and where did it go?** — the boundary's other half; name the
   nearest neighbours.
4. **Who maintains it?** — a named human.
5. **What types does it define, and what does each mean?** — one line per non-obvious
   type.
6. **What does it depend on, and why?** — every `[dependencies]` entry commented with
   the edge kind that needs it.
7. **What is known to be missing?** — a "Known gaps" section, so absence reads as *known
   and deferred*, not *overlooked*; `vaire unresolved` is the machine-readable half.

The dumping-ground test is question 2: **no three real questions → a folder, not a
package** — the material belongs in an existing boundary, or it is not yet curated
knowledge (records/drafts until it earns packagehood). The compounding test: every new
entity either helps answer an existing question or justifies adding one; an entity that
answers no question the package poses is in the wrong package.

## Growing the ontology — the type ladder

Types are load-bearing (ID namespace, hard filter in the entity pass, the class
observers query by), so they are expensive to retract — renaming one later is a MAJOR
with tombstones on every entity. The ontology grows reluctantly, by maintainer decision.
Work down; stop at the first rung that fits:

1. **Reuse an existing type as-is.** Check the whole workspace vocabulary first — the
   union of `types` across manifests (and any shared definitions package). Most new
   material fits.
2. **Reuse a type plus a frontmatter facet.** A *variant* is a field, not a type:
   one `goal` type with `kind: target` vs `kind: north-star` — two lenses, deliberately
   not two types, when no reference needs to target one lens as a class.
3. **Add a new type** — only when all four hold:
   - **Plural evidence:** several entities of the kind exist or are about to. One entity
     of a type is a definition wearing a namespace; three is a class.
   - **It is a retrieval class:** someone needs to address these *as a group* — a query,
     a `[[?type: …]]` guess, the pass's type-gated clustering — and no existing type's
     group is right.
   - **Distinct identity semantics:** the kind carries its own characteristic edge shape
     (its own set of typical frontmatter edges), not one an existing type implies.
   - **You can define it in one line** — and you write that line down, in the README and
     the manifest's declared `types`.

Rules of form: types are **singular, lowercase slugs** (`method`, not `methods`); a type
is declared by the package that defines its entities; two packages declaring the same
type name is fine (cross-package references are `@`-qualified, so names never compete);
and **never let a typo become an ontology** — an `unknown_type` warning is resolved by
quoting the value or deliberately declaring the type through this ladder, never by
letting it silently index.

## Moving knowledge out — boundary migration

When material belongs in another package's boundary:

1. **Create the successor** entity in the destination package (format per the
   **vaire-entity-authoring** skill), with the destination maintainer's agreement.
2. **Tombstone the original**: replace its content with frontmatter carrying
   `superseded_by: "@dest-pkg/type:id"` — never delete. The redirect keeps every
   existing reference resolving.
3. **Fix references in both directions**: the source package now needs a dependency on
   the destination for its tombstone (and any remaining edges); destination-side
   references back to the source stay legal — cycles between packages are allowed.
4. **Re-check both packages** (`vaire check`), then version: the move is MAJOR for the
   source (**vaire-versioning** skill).

## Don't

- Don't create a package for tidiness, without an owner, or below the substance bar.
- Don't add a type for one entity, or to mirror a source document's headings.
- Don't move knowledge out by delete-and-recreate — tombstones, always.
- Don't let the README's questions drift from what the entities can actually answer —
  auditing that list *is* package maintenance.
