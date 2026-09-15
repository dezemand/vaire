---
id: frontmatter-wikilink
type: finding
name: Frontmatter wikilink trap
aliases: [frontmatter_wikilink]
---
# Frontmatter wikilink trap

The classic muscle-memory mistake: writing inline-style `[[ ]]` bracket syntax inside a
frontmatter value, e.g. `head: [[person:jane]]`, where only bare values are meaningful (see
[[concept:frontmatter-edge]]). Unquoted, YAML parses the brackets into nested junk and the
field silently becomes not an edge; quoted, it becomes a harmless but meaningless string.
Either way, no [[concept:reference]] gets created, and nothing about that failure is loud
on its own.

Vairë forgivingly strips stray surrounding brackets so the reference still resolves
despite the typo, but `vaire check` warns regardless — the point isn't that the mistake was
fatal, it's that it must never be silent. The fix is simply to drop the brackets:
`head: person:jane` for a resolved reference, or `head: "?person: …"` (quoted for the
leading `?`) for a [[concept:loose-end]].
