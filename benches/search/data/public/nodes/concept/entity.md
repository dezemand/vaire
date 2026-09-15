---
id: entity
type: concept
name: Entity
---
# Entity

An entity is a [[concept:node]] with identity — a thing other nodes point *at* rather than
a thing that merely happened. People, systems, methods, packages, and concepts like this
one are all entities: each gets a stable typed ID and its own file, and each is **global**
— referenced from anywhere in the package, never duplicated per context. Contrast
[[concept:record]], which is scoped and describes an event rather than a subject.

Not every draft earns entity status. Four admission tests decide: a **single subject**
(you can say what it is *and* name something it deliberately excludes); a **type from the
vocabulary** (see [[concept:type-vocabulary]]) — no configured type, no entity; a
**substance floor** (two honest sentences is an alias or a section of its parent, not a
file, because a stub is a dangling promise wearing a permanent ID); and **standalone**
prose (no "as above," no unresolved acronyms, no unnamed actors).

## Why creation is gated

Entities are the one thing [[principle:gate-the-rare-act]] singles out. An ID gets
referenced everywhere once it exists, so a duplicate or a mis-merged entity poisons every
record that ever pointed at it — that is what makes minting one the rare, irreversible act,
handled once by the [[concept:entity-creation-pass]] rather than by whichever author
happens to be writing at the time. Everything else about authoring — writing records,
extending an entity's own prose, adding aliases — stays free precisely because none of it
can do that kind of damage.
