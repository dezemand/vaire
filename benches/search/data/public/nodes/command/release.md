---
id: release
type: command
scope: cli:vaire
name: vaire release
---
# vaire release

`vaire release [--major] [--dry-run] [--notes file] [--summary file] [--push]` cuts a
release end to end: the [[component:release-classifier]] computes the bump, the manifest
version is written, a [[concept:release-record]] is committed, and a tag is created — all
in one command, without the maintainer ever typing a version number except to escalate to
MAJOR deliberately.

MAJOR is gated behind both `--major` and a `--notes` file naming what it invalidates,
exiting its own dedicated code `7` rather than an ordinary failure, so a pipeline can tell
"awaiting a maintainer" apart from "broken." `--summary` accepts outside prose for the
record's narration while every structural field the release itself computed —
`added`/`changed`/`retired`/`bump` — stays refused to that file by name. Gated on a clean
tree, the mainline branch, and [[cli:vaire/command:check]] clean; nothing to release is a
clean no-op. It never runs `git push`, and never uploads unless `--push` is given —
uploading is [[cli:vaire/command:push]]'s job.
