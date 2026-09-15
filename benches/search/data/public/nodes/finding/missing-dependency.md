---
id: missing-dependency
type: finding
name: Missing dependency
aliases: [missing_dependency]
---
# Missing dependency

A [[concept:dependency]] is declared in `[dependencies]` but this machine can't actually
resolve it: nothing is linked at `.vaire/packages/<name>` yet, an existing link points at a
path that no longer answers, or the target directory's own manifest declares a different
`name` than expected. Reported exactly once per dependency name, always naming the specific
fix rather than a generic complaint.

Because this dependency's edges are skipped by the dangling-reference pass entirely while
it's missing, other findings can end up hiding behind it — this is why triage always
resolves every `missing_dependency` row first, before trusting any other report from the
same run. The fix follows the message verbatim: `vaire add <name> --link <path>` to wire it
explicitly, or getting the package discoverable through the [[concept:catalog]] so the
ordinary ensure pass can settle it on its own during the next
[[cli:vaire/command:index]].
