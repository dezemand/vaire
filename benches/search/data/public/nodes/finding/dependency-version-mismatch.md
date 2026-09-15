---
id: dependency-version-mismatch
type: finding
name: Dependency version mismatch
aliases: [dependency_version_mismatch]
---
# Dependency version mismatch

A [[concept:linked-package|linked]] dependency's own declared version has a MAJOR number
outside the constraint this package wrote for it in `[dependencies]` — the resolved package
says `2.x` where the manifest asked for `^1`, for instance. Surfaced only, deliberately;
enforcement of the constraint at resolution time is later work, so today this is strictly a
warning `vaire check` reports rather than something that blocks anything.

Two different situations produce this, and they call for different fixes: either the
dependency genuinely moved to a new major and this package owes the re-confirmation work
before raising its own `^N` to match, or the wrong checkout entirely got linked at
`.vaire/packages/<name>` and the link itself needs correcting. An *explicitly* linked
dependency is the only kind that can even land here — one the [[concept:catalog]] resolved
on its own can't, since the constraint is exactly what selected it in the first place.
