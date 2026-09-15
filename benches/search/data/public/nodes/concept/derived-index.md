---
id: derived-index
type: concept
name: Derived index
aliases: [disposable index]
---
# Derived index

The index at `.vaire/index.db` holds no truth of its own — every fact in it was woven from
the Markdown files, and every file under `.vaire/` is gitignored in full. That makes the
index **disposable**: deleting it and running [[cli:vaire/command:index]] again reproduces
it byte-for-byte-equivalent from the same commit, in seconds, because rebuilding is the
ordinary path rather than a disaster-recovery one.

This is the safe asymmetry behind [[principle:files-are-authoritative]] — a derived cache
cannot corrupt the truth, whereas an authoritative database would be something to back up
and defend. It is also why adopting a pre-1.0 storage engine ([[decision:turso-engine]]) is
low-risk: any regression is one `--full` rebuild away from a clean slate, never a
migration.
