---
id: release-classifier
type: component
name: Release classifier
---
# Release classifier

The piece of [[cli:vaire/command:release]] that computes a version bump instead of asking a
maintainer to type one. It diffs the entity-address set of the last released
[[concept:artifact]] — rebuilt from that release's own tag, since packing is deterministic
and nothing needs the old artifact retained — against the current tree, and maps what it
sees onto MINOR (addresses added), PATCH (content changed, same address set), or MAJOR
(addresses removed or a [[concept:supersession|tombstone]] appeared), taking the highest
that applies when several kinds of change are mixed together.

It deliberately compares only what a reader would notice — sections, edges, alias text —
so a moved file or a touched `updated:` timestamp never counts as a release on its own. It
also excludes its own output type from the diff entirely: without that exclusion, every
release would itself look like a MINOR addition, and no later release could ever classify
as a plain PATCH again.
