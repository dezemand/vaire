---
id: suggest
type: command
scope: cli:vaire
name: vaire suggest
aliases: [suggest command, lookup-before-reference]
---
# vaire suggest

`vaire suggest <descriptor> [--type T] [--limit N] [--local | --all]` is the
lookup-before-reference primitive every authoring contract in this corpus assumes: given a
free-text descriptor of something you want to reference, it returns ranked existing node
IDs it might be, matched against `name`/[[concept:alias|aliases]] first and prose full-text
only as a backup — no vectors at all, since bare embeddings are weak on short phrases.

A hit means write `[[type:id]]`; no plausible hit means write a
[[concept:loose-end|`[[?type: descriptor]]`]] instead and move on, never guess an ID. It's
also step three of the [[concept:entity-creation-pass]], matching each cluster of
descriptors against what already exists before deciding whether a new
[[concept:entity]] is warranted at all.
