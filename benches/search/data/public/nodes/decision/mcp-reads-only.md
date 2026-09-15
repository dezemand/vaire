---
id: mcp-reads-only
type: decision
name: MCP exposes only read commands
---
# MCP exposes only read commands

`vaire mcp` re-exposes exactly the eight read commands as MCP tools and nothing else —
[[cli:vaire/command:index]], `check`, `status`, and every other maintenance command are
absent from that surface by design, not by oversight. See
[[principle:reads-are-safe]] for the reasoning; this entry records the choice itself.

Because the MCP server and the CLI's read subcommands are one implementation exposed
twice, tool schemas mirror the CLI's own flags and every result is the identical `--json`
shape the CLI would print — there is no second serialization path that could quietly drift
from the first. If the [[concept:derived-index]] hasn't been built yet, a tool call returns
the same pointer toward `vaire index` that the CLI's exit code `4` gives a human; the
server is not permitted to trigger a build on an agent's behalf, ever.
