# Vairë

> A derived reference-graph index over a Markdown knowledge corpus — with a CLI and an MCP server.

Vairë turns a folder of Markdown into a queryable graph. You author plain `.md` files;
Vairë weaves their frontmatter and `[[wikilinks]]` into a derived SQLite index you can query
for backlinks, references, and search — from a shell or from an agent over MCP.

The core idea: **references are stable typed IDs, not display names.** Names change and
break links; IDs don't. Change an entity's `name:` once and every reference re-renders. The
files stay the source of truth — the index is a disposable cache, rebuildable in seconds and
never written back to the corpus.

> **Status:** early (0.1). The CLI and on-disk shapes are settling; expect changes.

## Install

**Prebuilt binary (Linux x86_64, macOS arm64).** Downloads the latest release and
installs it to `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/dezemand/vaire/main/install.sh | sh
```

On Windows (PowerShell), installs `vaire.exe` and adds it to your user PATH:

```powershell
irm https://raw.githubusercontent.com/dezemand/vaire/main/install.ps1 | iex
```

Both honor `VAIRE_VERSION` (a tag like `v0.1.0`) and `VAIRE_INSTALL_DIR` to override
the version and target directory.

**From source (Rust 1.85+)** — also the path for Intel macOS or arm64 Linux, which
have no prebuilt binary yet:

```bash
git clone https://github.com/dezemand/vaire.git && cd vaire
cargo install --path .        # installs the `vaire` binary
# or: cargo build --release   # → target/release/vaire
```

## Quickstart

```bash
vaire init my-notes && cd my-notes      # scaffolds .vaire/config.toml
# author some files (see "The model" below), then:
git init && git add -A && git commit -m "notes"
vaire index                              # build the index from the committed tree
vaire search "broker throughput"
vaire backlinks person:jane-doe
vaire suggest "the logistics team"       # descriptor → ranked existing IDs
```

While drafting, `vaire index --working-tree` indexes uncommitted edits so you can validate
before committing. There's a runnable corpus in [`example/`](example/) — `cd example &&
vaire --repo . index` and poke around.

## The model

A node is any `.md` file whose frontmatter has an `id:` and a `type:`; its address is the
composition **`type:id`**.

```markdown
---
id: jane-doe
type: person
name: Jane Doe
aliases: [Jane, J. Doe]
org: department:platform
---
# Jane Doe

Drives the [[method:event-sourcing]] rollout; owns [[system:ingest-api]].
```

- **Entities** (people, departments, methods, systems, …) are global and referenced by ID.
  **Records** (meeting notes, decisions, status) are project-scoped and additive.
- **References are IDs.** Frontmatter fields whose values are `type:id` become structured
  edges (`org`, `participants`, `references`, …); inline `[[type:id]]` are positional edges.
  `[[type:id|Display]]` overrides link text; `name:` (optional — falls back to the `# H1`
  then the filename) supplies it otherwise.
- **Loose ends.** Reference something not yet created with `[[?person: someone from ops]]`
  (inline) or `head: "?person: …"` (frontmatter) — a *descriptor*, never a guessed ID. These
  surface in `vaire unresolved` and never become edges.
- **Scoped IDs** (opt-in). With `scoped_types = ["record"]`, a record under a container is
  addressed `<container-id>/type:local`, e.g. `project:atlas/record:2026-06-10-standup`, so
  records only need a container-local id.

See [`spec/design.md`](spec/design.md) for the full design and rationale, and
[`spec/cli.md`](spec/cli.md) for the exact command surface.

## Commands

**Read** (also exposed over MCP) — every command takes `--json`:

| Command | Purpose |
| --- | --- |
| `resolve <id>` | Locate a node; follows `superseded_by` redirects. |
| `render <id>` | The node as portable Markdown, links resolved to `[name](path)`. |
| `backlinks <id>` | Nodes that reference `<id>`. |
| `refs <id> [--depth N]` | Nodes `<id>` references (traversable). |
| `search <query>` | Hybrid full-text + vector search. |
| `suggest <descriptor>` | Ranked existing IDs a descriptor might be (lookup-before-reference). |
| `unresolved` | Every `[[?…]]` loose end (the entity-creation work list). |

**Maintain** — `init`, `index` (`--full` / `--working-tree` / `--re-embed`), `check`
(`--strict`), `status`. Not exposed over MCP.

The index is bound to commit (commit-as-publish): `vaire index` reads the committed tree.
`vaire check` guards integrity — duplicate IDs and dangling references (failures); orphans,
drift, frontmatter-`[[ ]]`, and unknown-type references (warnings).

## Embeddings

Pluggable, **local by default** (a built-in, offline, dependency-free embedder), configured
in `.vaire/config.toml`:

- `provider = "local"` — built-in, no network, no model file.
- `provider = "command"` — shell out to any local model (JSON texts in, JSON vectors out).
- `provider = "openai"` — the OpenAI embeddings API; reads `OPENAI_API_KEY` from the
  environment or a gitignored `.vaire/.env`. Sends corpus text to OpenAI (data egress).

After switching models, `vaire index --re-embed` refreshes vectors without re-parsing.

## MCP server

```bash
vaire mcp --repo /path/to/corpus
```

Starts a STDIO Model Context Protocol server exposing the read commands as tools, one-to-one
— the tools *are* the CLI commands, so the surfaces can't drift. Maintenance commands are
not exposed; the agent surface is bounded to reads.

## Agent skills

[`skills/`](skills/) holds [Agent Skills](https://agentskills.io) that teach an agent the
file model and how to query Vairë via CLI or MCP: `vaire-files`, `vaire-query-cli`,
`vaire-query-mcp`.

## License

[MIT](LICENSE) © Maarten van Ittersum
