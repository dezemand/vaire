---
id: index
type: command
scope: cli:vaire
name: vaire index
---
# vaire index

`vaire index [--full] [--working-tree] [--re-embed] [--no-deps]` (re)builds the
[[concept:derived-index]]. In a Git repository with commits it reads the **committed**
tree by default and incrementally re-parses only what changed since the last build
([[concept:commit-as-publish]]); with no commits, or no Git repo at all, it falls back to a
full pass over the working tree instead. No read command ever builds the index as a side
effect — this is the only command that does.

`--full` forces a cold rebuild regardless of what's cached; `--working-tree` indexes
uncommitted edits for the author's own edit-validate loop, always recording a `null`
commit; `--re-embed` re-embeds every section under the current provider, bypassing the
[[decision:content-hash-embedding-cache]] on purpose. It also runs the dependency ensure
pass first (`--no-deps` skips it), refreshing every [[concept:linked-package|linked]]
dependency's own index through its link before touching this package's.
