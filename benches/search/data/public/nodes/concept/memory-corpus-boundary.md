---
id: memory-corpus-boundary
type: concept
name: Memory vs corpus boundary
aliases: [memory-corpus split]
---
# Memory vs corpus boundary

An agent's native memory — running logs, a consolidation pass, a promoted summary store —
is its own private, disposable scratch space. It answers "does the agent need this to work
better?" The corpus answers a different question entirely: "does something outside the
agent need to trust this?" Everything falls into exactly one bucket, and the two must never
mix.

The corpus is a workspace an agent **reads**, never data it feeds into its own memory
consolidation — that pipeline runs over interaction history, sessions and logs, never over
corpus files. It is read-as-truth and never promoted-from: a confirmed corpus fact must
never compete with the agent's own learned signal as if the two were the same kind of
evidence. At read time an agent may draw on both, but keeps provenance visibly distinct in
what it says back — a corpus fact is citeable to a [[concept:node|node]] address; a memory
signal is only ever the agent's own hunch. The membrane between the two is a single
deliberate act: writing a [[concept:record]]. There is no capture tier and no promotion
queue sitting in between (see [[decision:no-capture-tier]]).
