---
id: entity-creation-pass
type: concept
name: Entity-creation pass
aliases: [the gated pass]
---
# Entity-creation pass

The single gated process in an otherwise-autonomous corpus: it turns accumulated
[[concept:loose-end|loose ends]] into either a link to an existing
[[concept:entity]] or a brand-new one. Four steps, run on request rather than as a
background service: **scan** `vaire unresolved` for every `[[?...]]` currently in the
files; **cluster** descriptors by similarity, but only ever within the same type guess,
since a `?person` must never merge with a `?department`; **match** each cluster against
existing entities, aliases and full text first and embeddings only as backup; and
**resolve** — rewrite each reference additively (add the ID, keep the original phrasing as
display text) and commit.

It is deliberately **stateless**: the worklist is just whatever loose ends currently sit in
the files, so there is no queue to desync from reality and no cost to dying mid-run — a
rerun simply picks up wherever the corpus still shows open descriptors. A cluster that
matches one entity clearly, or matches none at all, resolves automatically; genuine
ambiguity — two plausible existing matches, or two clusters that might really be one — is
surfaced to a human rather than guessed at, which stays cheap precisely because entities are
created far less often than they're referenced.
