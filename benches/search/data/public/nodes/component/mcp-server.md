---
id: mcp-server
type: component
name: MCP server
---
# MCP server

`vaire mcp` starts a Model Context Protocol server over STDIO that re-exposes the eight
read commands as MCP tools, one-to-one, with input schemas mirroring each command's own
flags and results identical to that command's `--json` output. See
[[decision:mcp-reads-only]] for why maintenance commands never appear here.

One server instance always serves exactly one scope, resolved at startup the same way the
CLI resolves it — walking up to the nearest [[concept:manifest]], or, if none is found,
falling back to the whole [[concept:rootless-session|catalog]] instead. It never builds or
writes the [[concept:derived-index]] under any circumstances: if the index is missing, a
tool call returns the same pointer toward `vaire index` a human would see from exit code
`4`, and the agent driving it is never allowed to trigger a build on its own behalf.
