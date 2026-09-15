---
id: content-hash-embedding-cache
type: decision
name: Content-hash embedding cache
---
# Content-hash embedding cache

Every section's embedding is cached under `.vaire/` keyed by a hash of its own text, so an
ordinary [[cli:vaire/command:index]] re-embeds only the sections whose content actually
changed since the last build — a git diff narrows the changed files, and the cache narrows
further to the changed sections within them. A cold rebuild still re-embeds everything
exactly once.

Without this cache, "rebuildable in seconds" would stop being true the moment embeddings
entered the picture at all, since re-computing every vector on every reindex would make the
[[concept:derived-index]] expensive to regenerate rather than disposable. The cache is
intentionally blind to *which* provider produced a vector, though — swapping models or
providers requires `--re-embed`, which bypasses the cache on purpose so a reindex can never
quietly mix vectors from two different embedding spaces in one column.
