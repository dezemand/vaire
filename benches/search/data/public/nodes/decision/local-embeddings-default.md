---
id: local-embeddings-default
type: decision
name: Local embeddings by default
---
# Local embeddings by default

Embedding runs through a built-in local provider unless a maintainer opts into something
else — never an API call by default. Three reasons, all independent of each other: a
corpus may hold confidential material, and an API-backed provider means every section
leaks off-machine on every reindex; a tool that promises a rebuild in seconds cannot
depend on network availability; and re-embedding happens on *every* reindex, so a paid API
call per section would make routine use expensive by design.

Two opt-in alternatives exist for when local isn't enough: `command`, which shells out to
any locally running model, and `openai`, which accepts the network egress explicitly and
reads its key from the environment or `credentials.toml`. Whichever is chosen, embeddings
are configured per machine with [[?command: the configure embeddings command]], never in
the committed [[concept:manifest]] — a package must never dictate how whoever consumes it
chooses to index it.
