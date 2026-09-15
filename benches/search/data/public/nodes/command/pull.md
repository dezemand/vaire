---
id: pull
type: command
scope: cli:vaire
name: vaire pull
---
# vaire pull

`vaire pull [name[@^MAJOR|@version]] [--registry name] [--dry-run]` fetches a release into
the [[concept:store]]: verified against its digest, unpacked, re-indexed locally with the
puller's own tools rather than any shipped index, and sealed read-only. It is the **only**
command that ever acquires a package from the network — resolution itself never fetches
silently, so a dependency this machine can't already satisfy is reported together with the
exact `vaire pull` that would fix it, and a human or agent decides from there.

Bare `vaire pull` inside a package takes every declared dependency not yet satisfied;
`vaire pull <name>` works from anywhere, including a
[[concept:rootless-session|rootless]] one, which is what makes it the very first command
usable on a brand-new machine. When it replaces an existing version it reports the
[[concept:adopted-changes-digest]] — what changed that this package actually cites — rather
than the publisher's full changelog.
