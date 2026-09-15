---
id: artifact
type: concept
name: Artifact
---
# Artifact

The packed, distributable unit of a [[concept:package]] release:
`<name>-<version>.tgz`, built by [[cli:vaire/command:pack]] from the **committed** tree
only. It bundles the manifest, every file the include/exclude globs select, every file
those files reference (transitively, so a shipped document never carries a broken relative
link), and a freshly exported index.

Packing is reproducible by construction — sorted entries, timestamps pinned to the commit,
zeroed ownership, untimestamped compression, and nothing recording which build of `vaire`
packed it — so rebuilding the same tag anywhere produces byte-identical output, which is
exactly what lets [[cli:vaire/command:push]] rebuild an artifact from a git tag instead of
trusting a cached copy. Vectors ship by default (`--no-embeddings` strips them), but the
full-text index never ships at all: its on-disk form isn't portable between builds, so it's
always rebuilt fresh wherever the artifact lands.
