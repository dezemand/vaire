---
id: frontmatter-edge
type: concept
name: Frontmatter edge
---
# Frontmatter edge

A frontmatter edge is a [[concept:reference]] that lives in a node's structured metadata
rather than its prose: `owner: department:hr`, `participants: [person:jane-doe]`. It is
parsed without reading a word of the body, which is what lets tooling enumerate a node's
declared relationships cheaply and reliably — `grep '@'` for every cross-package edge is
the same trick applied to dependencies.

Frontmatter values are **bare** — no `[[ ]]` brackets, which are strictly an inline-prose
convention. Writing bracketed syntax in frontmatter is a common muscle-memory slip
([[finding:frontmatter-wikilink]]); Vairë strips stray brackets forgivingly but always
warns, because a silently swallowed edge is worse than an ugly one. An edge whose inline
counterpart exists but was never added to the frontmatter list produces the advisory
[[finding:drift]] instead — narrative-only mentions are allowed to stay inline-only, so
drift is a nudge, not a rule.
