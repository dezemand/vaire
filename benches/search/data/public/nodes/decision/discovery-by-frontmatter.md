---
id: discovery-by-frontmatter
type: decision
name: Discovery by frontmatter, not path
---
# Discovery by frontmatter, not path

A file is a [[concept:node]] because its frontmatter carries an `id:` and a `type:` — never
because of which directory it happens to sit in. The advisory directory layout exists
purely to help humans browse; Vairë itself never depends on it, so a file can move to any
path in the tree and every reference to it keeps working unchanged, since a reference was
never a path to begin with.

The trade accepted for this is that `type:` becomes the one authoritative source of a
node's type, and (for scoped nodes) the `scope:` field becomes the one authoritative
source of its container — neither is ever inferred from where a file lives. Discovery by
frontmatter is also what forces the two integrity checks that make ID-based addressing
trustworthy at all: every composed address must be unique ([[finding:duplicate-id]]), and
every resolved reference must point at something real ([[finding:dangling-ref]]).
