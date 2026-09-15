---
id: vaire
type: cli
name: vaire
aliases: [the vaire binary, vaire CLI]
---
# vaire

`vaire` is the single binary this whole corpus is about: one implementation, invoked as
`vaire [GLOBAL FLAGS] <command> [ARGS]`, with no second server-side copy anywhere to drift
out of sync — [[component:mcp-server|`vaire mcp`]] re-exposes a subset of the very same
subcommands as MCP tools rather than reimplementing them.

Every subcommand falls into exactly one of two classes. **Read** commands —
[[cli:vaire/command:search]], [[cli:vaire/command:suggest]],
[[cli:vaire/command:resolve]], [[cli:vaire/command:render]],
[[cli:vaire/command:backlinks]], [[cli:vaire/command:refs]],
[[cli:vaire/command:unresolved]], and [[cli:vaire/command:deps]] — only ever query the
already-built [[concept:derived-index]] and are the ones exposed over MCP (see
[[principle:reads-are-safe]]). **Maintain** commands —
[[cli:vaire/command:index]], [[cli:vaire/command:check]],
[[cli:vaire/command:status]], [[cli:vaire/command:release]],
[[cli:vaire/command:pack]], [[cli:vaire/command:push]], [[cli:vaire/command:pull]], and a
handful more — build the index, validate it, or touch distribution state, and are run only
by humans, hooks, or CI.

A handful of global flags apply everywhere: `--repo <path>` overrides root discovery,
`-o <fmt>`/`--output <fmt>` (or `VAIRE_OUTPUT`) picks `human`, `json`, or the token-cheaper
`toon` encoding, and `--frozen` restricts resolution to the [[concept:store]] only. Output
discipline is strict: stdout carries exactly one thing — the result, or, under a machine
format, the canonical error shape — and nothing else ever lands there, so a script never has
to guess which stream to parse.

Discovery walks up from the working directory to the nearest [[concept:manifest]] and binds
every subsequent command to that root; a bare `vaire` run with no manifest anywhere above it
becomes a [[concept:rootless-session]] instead, scoped to the whole
[[concept:catalog]] rather than to any one package. Exit codes are deliberately meaningful
past the usual `0`/`1` split — `6` for a `check` that found violations and `7` for a release
classified as MAJOR are both *outcomes* the run completed successfully, not failures, which
matters to anything branching on them in a pipeline.
