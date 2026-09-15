---
id: computed-bumps
type: decision
name: The version bump is computed, not typed
---
# The version bump is computed, not typed

A maintainer never types a version number for an ordinary release. `vaire release` diffs
the entity index of the last released [[concept:artifact]] against the current tree and
picks the bump from what it observes: new entity addresses with none removed is **MINOR**;
content changed with the address set identical is **PATCH**; any address removed, or a
[[concept:supersession|`superseded_by:`]] tombstone appearing, is **MAJOR**. Mixed changes
take the highest bump that applies.

MAJOR is deliberately never automatic — it exits with its own dedicated code and requires
both `--major` and a `--notes` file naming the invalidated assumptions dependents need to
read, because only a maintainer can own a claim about *meaning*, while the tool only ever
owns *structure*. The inverse holds too: `--major` can always escalate a textually tiny
edit, since reversing a stated truth is a semantic act no diff-based classifier could ever
detect on its own. A rename needs no special case in the classifier at all — because an
address *is* the identity, a rename simply presents as one removal plus one addition, and
the removal alone already forces MAJOR.
