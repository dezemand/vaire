---
id: render
type: command
scope: cli:vaire
name: vaire render
---
# vaire render

The one read command that returns a file **body** instead of pointers:
`vaire render <id>` emits the node's frontmatter verbatim and its prose with every
`[[type:id]]` [[concept:reference]] resolved to a standard `[display](relative-path)`
Markdown link — display text from the `|` override or the target's
[[concept:display-name]], falling back to its slug. It does, mechanically, exactly what a
reader would otherwise do by opening the file and following each link by hand.

An unresolved [[concept:loose-end]] renders as its bare descriptor, never as a link; a
dangling reference, and anything inside a fenced code block, is left completely verbatim.
Cross-package targets render with filesystem-relative hrefs through the link at
`.vaire/packages/<name>`.
