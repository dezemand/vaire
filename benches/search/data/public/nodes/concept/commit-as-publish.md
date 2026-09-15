---
id: commit-as-publish
type: concept
name: Commit as publish
aliases: [commit-is-publish]
---
# Commit as publish

The rule that a Git commit is the moment a record becomes real: when the corpus root is a
Git repository with commits, [[cli:vaire/command:index]] reads the **committed** tree, not
the working directory. Uncommitted edits are the author thinking out loud; a commit is
publication. Every index state therefore corresponds to exactly one commit, which is what
makes the derived index reproducible and diffable the same way the corpus itself is.

There is a deliberate escape hatch for the edit-validate loop:
`vaire index --working-tree` (and `vaire check --working-tree`) reads uncommitted changes
from disk instead, so an author can validate before committing anything. That mode always
records a `null` commit and is explicitly opt-in — a plain `vaire index` never silently
builds on top of working-tree state, so it can never inherit rows that were never
published. When the corpus isn't a Git repository at all, or has no commits yet, indexing
falls back to a full working-tree pass as the only option, and `null` just means "nothing
to bind to yet."
