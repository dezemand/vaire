---
id: reads-are-safe
type: principle
name: Reads are safe
---
# Reads are safe

Only the eight read commands — [[cli:vaire/command:resolve]],
[[cli:vaire/command:render]], [[cli:vaire/command:backlinks]], [[cli:vaire/command:refs]],
[[cli:vaire/command:search]], [[cli:vaire/command:suggest]],
[[cli:vaire/command:unresolved]], and [[cli:vaire/command:deps]] — are exposed over the MCP
server ([[component:mcp-server]]). Every maintenance command —
[[cli:vaire/command:index]], [[cli:vaire/command:check]],
[[cli:vaire/command:status]], `release`, and the rest — stays off that surface entirely,
run only by humans, git hooks, or CI.

The boundary isn't about trust in any one command; it's that an agent-facing tool surface
should be bounded to operations that cannot, structurally, corrupt anything. A read command
can consult the [[concept:derived-index]] and open files for depth, but it can never build
an index, write a corpus file, or touch `.vaire/`. Because the CLI and the MCP tools are the
same implementation exposed twice, there is nothing to keep in sync and nothing quietly
broader on one surface than the other.
