# Vairë

> A derived reference-graph index over a Markdown knowledge corpus — with a CLI and an MCP server.

Vairë turns a folder of Markdown into a queryable graph. You author plain `.md` files;
Vairë weaves their frontmatter and `[[wikilinks]]` into a derived index you can query
for backlinks, references, and search — from a shell or from an agent over MCP.

The core idea: **references are stable typed IDs, not display names.** Names change and
break links; IDs don't. Change an entity's `name:` once and every reference re-renders. The
files stay the source of truth — the index is a disposable cache, rebuildable in seconds and
never written back to the corpus.

> **Status:** early (0.3). The CLI and on-disk shapes are settling; expect changes.

A corpus is a **knowledge package** (`knowledge.toml`), and packages reference each other:
declare a dependency (`vaire add acme-core`) and `@acme-core/team:platform` resolves,
searches, and lints across the boundary. See [`examples/workspace/`](examples/workspace/).

Packages are matched by the name their manifest **declares**, at any depth, so a knowledge
base living inside a bigger repo is found like any other. Two packages declaring the same
name are reported rather than guessed between, and `vaire add <pkg> --link <path>` wires
one explicitly (an explicit link always wins).

**0.3 adds distribution.** A package can be released, published to a registry, and pulled
by somebody who has never seen your checkout — with a static file host as a full citizen of
the protocol, so a directory or an S3 bucket is a working registry:

```bash
vaire release                            # the version is computed, not typed
vaire push --registry lab                # upload what the registry lacks
# elsewhere:
vaire add acme-core && vaire pull        # fetch it, verify it, rebuild its index here
```

Nothing is fetched behind your back: resolution never reaches the network, and a
dependency this machine cannot satisfy is reported with the `vaire pull` that would fix
it. See [Distribution](#distribution) below.

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

Both honor `VAIRE_VERSION` (a version like `0.1.0`; a leading `v` is accepted) and
`VAIRE_INSTALL_DIR` to override the version and target directory. Installing the
latest is a no-op when the installed `vaire` is already at or above it; a pinned
`VAIRE_VERSION` always installs.

Each release publishes a `SHA256SUMS` asset, and both installers verify the archive
against it before extracting — HTTPS authenticates the transport, not the artifact.
A mismatch aborts the install. Set `VAIRE_SKIP_CHECKSUM=1` to bypass the check, and
verify a manual download with:

```bash
sha256sum --check --ignore-missing SHA256SUMS
```

**From source (Rust 1.88+)** — also the path for Intel macOS or arm64 Linux, which
have no prebuilt binary yet:

```bash
git clone https://github.com/dezemand/vaire.git && cd vaire
cargo install --path .        # installs the `vaire` binary
# or: cargo build --release   # → target/release/vaire
```

**Upgrading.** An installed binary updates itself to the latest release
(`vaire upgrade --check` only reports; a cargo-installed binary is left to
`cargo install`):

```bash
vaire upgrade
```

## Quickstart

```bash
vaire init my-notes && cd my-notes      # scaffolds knowledge.toml
# author some files (see "The model" below), then:
git init && git add -A && git commit -m "notes"
vaire index                              # build the index from the committed tree
vaire search "broker throughput"
vaire backlinks person:jane-doe
vaire suggest "the logistics team"       # descriptor → ranked existing IDs
```

While drafting, `vaire index --working-tree` indexes uncommitted edits so you can validate
before committing. There are two runnable examples in [`examples/`](examples/) — a single
package in [`examples/corpus/`](examples/corpus/) (`cd examples/corpus && vaire --repo .
index` and poke around), and a three-package workspace in
[`examples/workspace/`](examples/workspace/).

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
- **Scoped IDs** (data-driven). A node carrying `scope: <container-id>` is addressed
  `<container-id>/type:local`, e.g. `project:atlas/record:2026-06-10-standup`, so records only
  need a container-local id. Any type can be scoped; `scoped_types_whitelist`/`blacklist` are a
  `vaire check` lint policy, not a gate.

See [`spec/design.md`](spec/design.md) for the full design and rationale,
[`spec/cli.md`](spec/cli.md) for the exact command surface,
[`spec/manifest.md`](spec/manifest.md) for the `knowledge.toml` package manifest, and
[`spec/registry.md`](spec/registry.md) for release, distribution, and the registry wire
contract.

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
| `deps` | The resolved dependency tree (live link inspection; no index needed). |

Run a read command where there is **no package above you** and the scope becomes every
package this machine knows — `vaire mcp` outside a package serves exactly that, which is
how an agent gets pointed at the machine rather than at one checkout. From inside a
package, `search --all` / `suggest --all` reach past the closure the same way.

**Maintain** — not exposed over MCP:

| Command | Purpose |
| --- | --- |
| `init`, `add` | Scaffold a package; declare a dependency. |
| `index` (`--full` / `--working-tree` / `--re-embed`) | Build the index. |
| `check` (`--strict`) | The integrity guards. |
| `status` | Index state, the pending release, dependency drift. |
| `catalog add \| rm \| list \| scan` | What packages this machine knows and where they live. |
| `release` (`--major` / `--summary`) | Cut a release: classify, version, record, commit, tag. |
| `pack` | Build the distributable artifact. |
| `registry add \| rm \| list \| show` | The registries this machine publishes to and pulls from. |
| `push`, `yank` | Upload releases; withdraw one from new adoption. |
| `pull` (`--locked`) | Fetch a release into the store. |
| `pin`, `unpin` | Hold a dependency at one exact version. |
| `clean` (`--dry-run`) | Drop store entries nothing needs. |
| `configure`, `upgrade` | User settings; update the binary. |

The index is bound to commit (commit-as-publish): `vaire index` reads the committed tree.
`vaire check` guards integrity — duplicate IDs and dangling references (failures); orphans,
drift, frontmatter-`[[ ]]`, and unknown-type references (warnings).

## Distribution

Releasing is one command, and **the version is computed rather than typed**: the classifier
diffs the last released artifact's entity index against your tree. Entities added is a
MINOR, sections edited is a PATCH, and anything removed or renamed is a MAJOR — which is
gated, because a truth reversal is a semantic act and only a maintainer can own it.

Each release also writes an **entity describing itself**, carrying edges to what it added,
changed and retired. Because the changelog is corpus, "which releases touched this entity?"
is an ordinary `vaire backlinks` query — and when you advance a dependency, `vaire pull`
reports what changed *that your package actually cites*, rather than the publisher's whole
changelog.

Pulled releases land in a **store** under `~/.vaire`, unpacked and sealed read-only. The
shipped index is treated as a claim, never as truth: materialization throws it away and
rebuilds from the shipped Markdown with your own vaire, so your answers never depend on a
stranger's build.

Two rules are worth knowing before you rely on any of it:

- **A working copy outranks a pulled release of the same name.** A checkout is what you are
  authoring, and resolving to a published copy of it would quietly answer against yesterday.
- **Only a store-resolved answer is reproducible.** `knowledge.lock` records which is which
  — an entry with a checksum can be fetched again anywhere; one without came from a
  checkout that has no artifact to checksum. `vaire pull --locked` reproduces the recorded
  resolution, and the global `--frozen` refuses anything that is not reproducible, which is
  the posture agents and CI want.

`vaire pin acme-core@1.4.1` holds an exact version against all of that; `vaire clean`
reclaims what no lockfile, pin, or standing request needs.

## Embeddings

Pluggable, **local by default** (a built-in, offline, dependency-free embedder). Embeddings
are machine-local config — not part of a package manifest — set with `vaire configure` and
stored in the user config (see `spec/cli.md` §6.3):

- `provider = "local"` — built-in, no network, no model file.
- `provider = "command"` — shell out to any local model (JSON texts in, JSON vectors out).
- `provider = "openai"` — the OpenAI embeddings API; reads `OPENAI_API_KEY` from the
  environment or `credentials.toml` (`printf '%s' "$OPENAI_API_KEY" | vaire configure
  embeddings --api-key-stdin`). Sends corpus text to OpenAI (data egress).

After switching models, `vaire index --re-embed` refreshes vectors without re-parsing.

## MCP server

```bash
vaire mcp --repo /path/to/corpus
```

Starts a STDIO Model Context Protocol server exposing the read commands as tools, one-to-one
— the tools *are* the CLI commands, so the surfaces can't drift. Maintenance commands are
not exposed; the agent surface is bounded to reads.

Run it **outside any package** and it serves every package this machine knows, with no
manifest and no install:

```bash
vaire catalog scan ~/Documents/Knowledge   # record what you have, once
vaire mcp                                  # serve all of it
```

## Agent skills

[`skills/`](skills/) holds [Agent Skills](https://agentskills.io) that teach an agent the
file model, how to query Vairë over CLI or MCP, and how to author against a corpus without
overstepping — reference (`vaire-files`, `vaire-packages`, `vaire-query-cli`,
`vaire-query-mcp`) and situational (answering, contributing, entity authoring and creation,
check triage, package curation, versioning). See [`skills/README.md`](skills/README.md) for
which role each one is for.

## License

[MIT](LICENSE) © Maarten van Ittersum
