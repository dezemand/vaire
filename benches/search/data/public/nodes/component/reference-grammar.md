---
id: reference-grammar
type: component
name: Reference grammar
aliases: [target grammar]
---
# Reference grammar

The parser that decides, by shape alone, whether a string could possibly be a
[[concept:reference]] at all — before anything consults configuration. The charset is
deliberately narrow (`[a-z][a-z0-9-]*` for a type, `[a-z0-9][a-z0-9-]*` for an id, joined by
a colon, optionally prefixed with `@package/` and chained with `/` for
[[concept:scoped-id|scoped]] addresses) specifically so URLs, email addresses, timestamps,
and version numbers are structurally excluded rather than needing to be special-cased one
at a time.

This is the fix for what the project's own history calls the original `url:` bug: treating
identification (does this look like a reference?) and classification (is its type actually
declared in [[concept:type-vocabulary|`types`]]?) as one conflated question meant a
field like `url: https://…` could accidentally be mistaken for one. Splitting them means a
value that fails the grammar is simply a plain scalar, full stop — no config ever gets
consulted for it, and no warning ever fires on it either.
