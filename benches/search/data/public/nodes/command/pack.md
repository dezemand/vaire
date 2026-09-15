---
id: pack
type: command
scope: cli:vaire
name: vaire pack
---
# vaire pack

`vaire pack [--no-embeddings]` builds the distributable [[concept:artifact]] —
`.vaire/dist/<name>-<version>.tgz` — from the committed tree only: the manifest, every
file the include/exclude globs select, every file those transitively reference, and a
freshly exported index. It's the publication gate as much as a build step: it refreshes
the index to HEAD and runs the full [[cli:vaire/command:check]] suite first, and any
violation refuses the pack outright rather than shipping broken references to every future
consumer.

Output is bit-for-bit reproducible — sorted entries, commit-pinned timestamps, zeroed
ownership, untimestamped compression — which is precisely what lets
[[cli:vaire/command:push]] rebuild the identical artifact from a bare git tag later,
without ever needing this command's own output retained anywhere.
