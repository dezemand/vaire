---
id: unreferenceable-id
type: finding
name: Unreferenceable ID
aliases: [unreferenceable_id]
---
# Unreferenceable ID

A node's own declared `id:` or `scope:` value falls outside the reference target grammar —
an uppercase letter, an underscore, anything the strict `[a-z0-9][a-z0-9-]*` charset
doesn't allow (`id: Jane_Doe`). The file still indexes fine, because
[[principle:files-are-authoritative]] holds regardless, but nothing anywhere can ever
successfully address it with a `[[type:id]]` reference — a silent dead end that would
otherwise go completely unnoticed until someone tried and failed to link to it.

The fix, applied *before* anything ever references it: correct the slug to something
legal — lowercase letters, digits, hyphens only. If the badly-formed ID was already
published and something out there might already be citing it verbatim as text, give it a
correctly-slugged successor and leave the old file behind purely as a
[[concept:supersession|tombstone]], rather than renaming it in place.
