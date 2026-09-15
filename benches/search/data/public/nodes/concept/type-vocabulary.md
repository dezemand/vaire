---
id: type-vocabulary
type: concept
name: Type vocabulary
---
# Type vocabulary

The `types` list in `knowledge.toml` is the set of `type:` values a [[concept:package]]
defines — simultaneously the ID namespace for every `type:id` address and the gate that
decides whether a frontmatter value shaped like a [[concept:reference]] becomes a
[[concept:frontmatter-edge]] or is silently ignored ([[finding:unknown-type]]).
Identification of a reference is by shape alone and needs no config; only this second step,
classification, ever consults the vocabulary.

Growing it is meant to be reluctant. A new type is expensive to retract later — renaming
one is a MAJOR with a tombstone on every affected entity — so the guidance for a maintainer
is a ladder: reuse an existing type as-is first, reuse one plus a frontmatter facet second,
and only add a genuinely new type when several entities of the kind already exist, someone
needs to address them as a retrieval class, and the kind carries its own characteristic
edge shape. Two packages may declare the same type name freely; it never collides, because
every cross-package reference is explicitly `@`-qualified.
