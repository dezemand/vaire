---
id: hybrid-search
type: component
name: Hybrid search
aliases: [FTS + vector search]
---
# Hybrid search

The ranking layer behind [[cli:vaire/command:search]]: full-text and alias matching run
first, with vector similarity layered underneath purely for recall. This split follows from
what each retrieval job actually needs — backlinks and graph traversal want no vectors at
all, [[?component: reference resolution matching]] wants aliases and full text because bare
embeddings are weak on short descriptors ("the broker thing" embeds to mush), and only
genuinely open-ended search benefits from vectors at all, because that's the one case where
a caller's phrasing won't lexically match anything in the corpus.

Results are ordered by descending score with ties broken by ID for determinism, and a
cross-package query embeds the text once and reuses it across every member of the
dependency closure. A dependency whose index was built with different embedding dimensions
still contributes its full-text and alias hits; only its vector recall silently drops,
which [[cli:vaire/command:status]] is what surfaces the mismatch.
