---
id: check
type: command
scope: cli:vaire
name: vaire check
aliases: [integrity check]
---
# vaire check

`vaire check [--strict] [--working-tree] [--no-deps]` runs every integrity guard
ID-based discovery makes possible and exits non-zero on any violation, which is what makes
it work as a pre-commit hook or a CI gate. Four kinds always block:
[[finding:duplicate-id]], [[finding:dangling-ref]], [[finding:undeclared-import]], and
[[finding:missing-dependency]]. The rest — [[finding:drift]], [[finding:orphan]],
[[finding:frontmatter-wikilink]], [[finding:unknown-type]],
[[finding:unreferenceable-id]], [[finding:malformed-diagram-ref]],
[[finding:scoped-type-not-permitted]], [[finding:unused-dependency]], and
[[finding:dependency-version-mismatch]] — are warnings that `--strict` promotes to
failures, the posture expected before any release.

`--working-tree` reindexes uncommitted edits first, turning this into the agent's
edit-then-validate loop without a commit in between. A clean `vaire check` is, by
convention, the definition of done for any change to the corpus.
