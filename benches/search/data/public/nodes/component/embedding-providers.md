---
id: embedding-providers
type: component
name: Embedding providers
aliases: [embedding backends]
---
# Embedding providers

The pluggable `embed(texts) → vectors` seam behind every section's vector. Three
implementations ship: `local`, a built-in offline model used unless something else is
configured (see [[decision:local-embeddings-default]]); `command`, which shells out to any
locally running model over `sh -c`, exchanging JSON texts for JSON vectors; and `openai`,
which calls the OpenAI embeddings API and therefore sends corpus text off-machine, gated
behind an explicit `OPENAI_API_KEY`.

Which provider is active is machine and consumer configuration, set with
[[?command: the configure command]], never a setting in the committed
[[concept:manifest]] — a package's authors don't get to dictate how a downstream reader
indexes their own copy of it. Switching providers, or changing dimensions, requires
`--re-embed` on the next [[cli:vaire/command:index]], which bypasses the
[[decision:content-hash-embedding-cache]] on purpose so old and new vectors are never mixed
in one column.
