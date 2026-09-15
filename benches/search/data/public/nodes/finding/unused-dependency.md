---
id: unused-dependency
type: finding
name: Unused dependency
aliases: [unused_dependency]
---
# Unused dependency

A [[concept:dependency]] sits in `[dependencies]` but nothing in this package's files
actually references it with an `@pkg/type:id` [[concept:reference]]. A warning, not a
violation — an unused entry costs nothing to leave alone, unlike a
[[finding:missing-dependency]], which actively blocks resolution of real references.

Ordinarily the fix is simply to remove the entry, keeping the manifest honest about what's
actually in use. The one legitimate exception is a dependency added deliberately ahead of
references that are coming soon but don't exist yet — in that case, keep the entry, ideally
commented so a future reader understands why it's there rather than assuming it's stale
cruft nobody got around to deleting.
