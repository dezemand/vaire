---
id: corpus-scanner
type: component
name: Corpus scanner
---
# Corpus scanner

Walks a package's `include`/`exclude` globs and classifies each file it finds: does its
frontmatter carry both an `id:` and a `type:`? If so it's a [[concept:node]], indexed under
its composed address; if not, it's prose or payload the index simply skips over (optionally
still searchable full-text, never addressable). The same scanning logic backs
`vaire catalog scan`, which walks a directory tree once to bulk-import every package it
finds into the [[concept:catalog]].

Because classification runs purely off frontmatter rather than directory conventions (see
[[decision:discovery-by-frontmatter]]), the scanner never needs to know anything about a
package's advisory layout — `knowledge/`, `projects/`, or any other arrangement of folders
scans identically. It's also what feeds [[component:reference-grammar|reference
parsing]]: each scanned node's frontmatter values and inline wikilinks are handed onward to
be identified and classified as edges or loose ends.
