---
id: turso-engine
type: decision
name: Turso as the index engine
---
# Turso as the index engine

The [[concept:derived-index]] runs on Turso Database — the ground-up Rust rewrite of
SQLite — embedded as a local file, never a server and never reachable over the network.
The reason is narrow and concrete: native full-text search and native vector columns in
one engine, so [[component:hybrid-search]] needs no separate FTS library bolted to a
separate vector store.

A few thousand sections is small enough that exact cosine distance
(`vector_distance_cos`) runs sub-millisecond with no approximate-nearest-neighbor index at
all — ANN support arrives for free later if Turso ships it, at no cost paid now. Because
[[principle:index-is-disposable]] holds regardless of what's underneath it, adopting a
pre-1.0 engine carries little risk: any regression it introduces is one `--full` rebuild
away from disappearing.
