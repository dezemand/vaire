# Examples

Two runnable examples, each with its own README and a test that keeps it honest
(`tests/release_gate.rs` builds both with the real commands, so a shipped example cannot
rot silently).

| | What it shows |
| --- | --- |
| [`corpus/`](corpus/) | **Start here.** One package: the file model, typed `type:id` addresses, frontmatter edges and inline wikilinks, loose ends, and what `check` reports. |
| [`workspace/`](workspace/) | Three packages referencing each other across boundaries: `@pkg/type:id` resolution, linked dependencies, cross-package tombstones, and the resolution lints. |
