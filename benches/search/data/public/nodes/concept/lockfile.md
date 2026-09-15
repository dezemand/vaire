---
id: lockfile
type: concept
name: Lockfile
---
# Lockfile

`knowledge.lock` is written by `vaire pull` and by the dependency ensure pass — never by
hand — and it records the *whole* resolved closure, because reproducing one dependency
means reproducing all of them consistently. Each entry says how its package resolved, and
only one kind is a reproducibility claim: a `registry` entry carries the version, the
registry name, and the artifact's `sha256`, so `pull --locked` can fetch those exact bytes
anywhere; a `workspace` entry — a plain checkout — records only a version, because a
working copy has nothing to checksum and can change between two runs.

That asymmetry is deliberate rather than a gap: writing a digest for a checkout would be a
promise the tool cannot keep. `vaire --frozen` is what turns the record into enforcement,
answering only from the [[concept:store]] and refusing any dependency that would otherwise
resolve to a [[decision:working-copy-outranks-release|working copy]] — the posture CI and
agents want, since a stale lock is merely imprecise rather than unsafe.
