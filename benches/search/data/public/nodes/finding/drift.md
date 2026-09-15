---
id: drift
type: finding
name: Frontmatter/inline drift
aliases: [drift]
---
# Frontmatter/inline drift

A resolved inline `[[type:id]]` wikilink whose target never made it into the node's
frontmatter edge list. Unlike most warnings, this one names a specific, actionable
direction to fix in: add the missing [[concept:frontmatter-edge]], never remove the inline
link.

It stays a warning rather than a violation on purpose — narrative prose legitimately
mentions things a structured edge list was never meant to enumerate exhaustively, so not
every inline mention should become a formal edge. The judgment call is whether the relation
is structural (belongs in frontmatter alongside `owner`, `participants`, and friends) or
purely narrative (fine to leave inline-only). `--strict` promotes it to a failure for
release gates, where a maintainer is expected to have made that call deliberately rather
than left it as a passive gap.
