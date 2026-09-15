---
id: search
type: command
scope: cli:vaire
name: vaire search
aliases: [search command]
---
# vaire search

`vaire search <query> [--type T] [--scope container-id] [--limit N] [--local | --all]`
runs [[component:hybrid-search]] over the corpus and returns whole files with the matching
section anchors — never file bodies, per the pointers-not-prose rule every read command
follows. Results default to a limit of 10, ranked by descending score with ties broken by
ID.

It's the tool behind open-ended, phrasing-varies retrieval — "what does the corpus say
about X" — as distinct from [[cli:vaire/command:suggest]], which is the sharper
lookup-before-reference tool for turning a short descriptor into a candidate ID before
writing `[[type:id]]`. `--scope` restricts to nodes under one
[[concept:scoped-id|container]]; `--all` widens past the current package to every package
the [[concept:catalog]] knows, the same fan-out a [[concept:rootless-session]] gets by
default.
