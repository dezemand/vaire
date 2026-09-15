---
id: catalog
type: concept
name: Catalog
---
# Catalog

The catalog (`~/.vaire/catalog.db`) is what one machine knows about the
[[concept:package|packages]] it has seen and where they live — an inventory, never an
authority. Each row is a **sighting**: "a package declaring name *N* at version *V* was
observed at path *P*," keyed by the canonicalized path so two routes to one directory can
never register twice. A sighting is `live` or `missing`, checked on read rather than
expired on any timer, and flips back to `live` the moment a vanished path answers again.

Registration is mostly **ambient** — `vaire index`, `check`, and `add` record what they
touch without any extra ceremony — with `vaire catalog scan <dir>` as the deliberate bulk
import for a tree you already have. Because the catalog is only ever a rebuildable index
over manifests that are re-read before anything resolves from them, losing the file costs a
rescan and nothing more — except for the rows nothing else can reproduce, like pins and
requested pulls, which is why a corrupt catalog is set aside rather than silently deleted.
