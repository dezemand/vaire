---
id: resolve
type: command
scope: cli:vaire
name: vaire resolve
---
# vaire resolve

`vaire resolve <id>` locates one [[concept:node]] by its exact address and returns its
path, type, and frontmatter — the sharpest possible lookup when the ID is already known,
in contrast to [[cli:vaire/command:search]] or [[cli:vaire/command:suggest]], which exist
for when it isn't. It follows [[concept:supersession|`superseded_by`]] redirects
transparently and reports the chain it walked, so a request for a retired ID still answers
with the winner rather than failing.

Exit code `5` means the ID plainly doesn't exist; `4` means its package is declared but
unavailable. A cross-package `@pkg/type:id` resolves through that package's own
dependencies, never the caller's.
