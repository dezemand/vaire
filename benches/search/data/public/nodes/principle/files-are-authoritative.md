---
id: files-are-authoritative
type: principle
name: Files are authoritative
---
# Files are authoritative

The Markdown files in a corpus are the only source of truth; the [[concept:derived-index]]
is a cache woven from them and nothing more. It never writes the corpus back, which is the
safe asymmetry the whole system leans on: a broken or stale index is merely inconvenient,
rebuildable in seconds, while a broken corpus file would be a real loss.

This is also why the index can adopt an experimental storage engine
([[decision:turso-engine]]) at low risk, why a pulled release's shipped index is thrown away
and rebuilt rather than trusted (see [[decision:shipped-index-not-trusted]]), and why two
retrieval paths coexist deliberately — authors open files and follow `[[wikilinks]]`
directly for depth, while the index answers only the queries files are bad at (backlinks,
full-text, semantic recall). The index augments file-native reading; it never replaces it.
