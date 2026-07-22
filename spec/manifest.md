# `knowledge.toml` — the package manifest

The committed manifest at a package root. It declares the package's identity, the file scope
that *is* the package, the entity types it defines, and the other packages it depends on. It
is the one authored, version-controlled config file; everything under `.vaire/` is derived and
gitignored (design.md §9).

`knowledge.toml` replaces the older `.vaire/config.toml` — see §8 for how `vaire init`
migrates an existing corpus.

## 1. Location and discovery

`knowledge.toml` lives at the **package root** and is what marks a directory as a Vairë
package. Discovery walks up from the working directory to the nearest ancestor containing a
`knowledge.toml` (precedence: `--repo` > `VAIRE_REPO` > walk-up). An explicit `--repo`/
`VAIRE_REPO` path that lacks one is an error, not a silent guess.

`.vaire/` sits beside the manifest and holds only the derived index (`.vaire/index.db`) plus a
self-contained `.vaire/.gitignore`. It is no longer a corpus marker.

```text
my-package/
├── knowledge.toml      # committed manifest — the package marker
├── knowledge/…         # entity files
└── .vaire/             # derived, gitignored (index)
```

## 2. The manifest

```toml
# Identity — required.
name    = "acme-web"          # package id; a slug, declared, never derived from the directory
version = "1.4.2"             # semver MAJOR.MINOR.PATCH
description = "The web application package"   # optional, one line

# File scope. The typed `id:`+`type:` pair is still what makes a file a node; these globs
# only bound the search space.
include = ["knowledge/**/*.md", "projects/**/*.md"]
exclude = ["**/node_modules/**", "**/drafts/**", "**/archive/**"]

# The entity types this package DEFINES (the `type:` field, also the ID prefix in `type:id`).
# Growable; an unlisted prefix is still indexed, but `vaire check` warns when
# `vocabulary_strict` is set.
types = ["service", "component"]
vocabulary_strict = false

# Scoped IDs. Scoping is DATA-DRIVEN: any node carrying the `scope_field` gets the composed
# address `<container-id>/<type>:<local-id>`, regardless of type (cli.md §6.1). The two lists
# don't change that — they are a lint policy: `vaire check` warns when a scoped node's type is
# not permitted. A type is permitted iff it matches the whitelist and not the blacklist; `"*"`
# matches any type. Defaults permit everything (no warnings).
scoped_types_whitelist = ["*"]
scoped_types_blacklist = []
scope_field            = "scope"   # frontmatter field naming the container, e.g. `scope: project:atlas`

# Dependencies on other packages: `name → "^MAJOR"`. See §5.
[dependencies]
acme-core = "^1"
acme-ui   = "^2"
```

A minimal manifest is just identity:

```toml
name    = "acme-core"
version = "1.0.0"
```

## 3. Field reference

| field | required | type | default | notes |
|---|---|---|---|---|
| `name` | **yes** | slug | — | package identity; declared, not path-derived |
| `version` | **yes** | semver | — | `MAJOR.MINOR.PATCH` |
| `description` | no | string | *(none)* | one line |
| `include` | no | glob[] | `["knowledge/**/*.md", "projects/**/*.md"]` | package file scope |
| `exclude` | no | glob[] | `["**/node_modules/**", "**/drafts/**", "**/archive/**"]` | |
| `types` | no | slug[] | *(empty)* | the entity types this package **defines** |
| `vocabulary_strict` | no | bool | `false` | when set, `vaire check` warns on a type not in `types` |
| `scoped_types_whitelist` | no | slug[] | `["*"]` | lint: types permitted to be scoped (`"*"` = any) |
| `scoped_types_blacklist` | no | slug[] | *(empty)* | lint: types **not** permitted to be scoped (`"*"` = none) |
| `scope_field` | no | string | `"scope"` | frontmatter field carrying the container id |
| `[dependencies]` | no | table `name → "^MAJOR"` | *(empty)* | see §5 |

All keys except `name`/`version` are optional; their defaults make a single-package corpus
work from a two-line manifest.

> **`types` default.** A manifest that omits `types` defines **none** — the field defaults to
> empty, so declaring nothing means the package exports no vocabulary. (This differs from the
> internal `Config::default()`, which carries a starter vocabulary for unconfigured use.)

## 4. Validation

Loading a manifest validates it; the first violation is reported with its reason.

- **`name`** must be a slug matching `[a-z][a-z0-9-]*` (lowercase, starts with a letter).
- **`version`** must be `MAJOR.MINOR.PATCH`, each a non-empty run of digits.
- Each **dependency constraint** must be the `^MAJOR` form (see §5).

## 5. Dependencies

`[dependencies]` maps a package `name` to a version constraint. **`^MAJOR` is the only legal
form** (`^1`, `^2`, …): a dependent matches a major line and adopts its minor/patch updates
automatically. Tighter pins and ranges are rejected — minor and patch changes never break
references by definition, so pinning would only create churn.

Add a dependency with **`vaire add <name>[@^N]`** (default `^1`) — it edits `[dependencies]`
in place, preserving your formatting and comments (cli.md §4.2a). Dependency *resolution*
(locating the depended-on packages and resolving cross-package references) is a separate
concern layered on top of this file; the manifest only declares the constraints.

A reference to another package is written `@<name>/<type>:<id>` (design.md §6) and must name
a declared dependency, resolvable through the linked-package lookup — the referencing
package's own `.vaire/packages/<name>`, the run-root itself, or the run-root's links
(cli.md §6.5). `vaire check` enforces the full set (packages.md §8):

| finding | severity | when |
|---|---|---|
| **undeclared import** | error | an `@pkg/…` reference whose package is not in `[dependencies]` (a pure table check — no resolution needed) |
| **dangling cross-package** | error | a declared, linked `@pkg/…` reference whose target — after tombstone-following in the owning package — does not exist |
| **missing dependency** | error | a declared dependency that is unavailable (not linked / broken link / name mismatch); once per name, with the fix |
| undeclared type | warning | a value matching the reference grammar whose `type` is not in `types` (quote it, or declare the type) |
| unused dependency | warning | declared in `[dependencies]` but never referenced |
| version mismatch | warning | a linked dependency whose MAJOR falls outside the `^N` constraint — surfaced only; *enforcement* is v0.3 |

`vaire deps` (cli.md §3.8) prints the resolved tree these constraints declare.

## 6. Machine and consumer settings are not here

`knowledge.toml` is the package's *published contract*. Anything a consumer or a machine
decides for itself — notably **embeddings** (provider, model, dimensions) — is not part of it,
because how a corpus is indexed is a consumer choice, not a property of the package. Those
settings live in the **global user config**, set with `vaire configure` (cli.md §6.3); secrets
go in a `600` `credentials.toml` (cli.md §6.2). A legacy `[embeddings]` table in a manifest is
ignored (and `vaire init` drops it when migrating, §8) — do not add it to new manifests.

## 7. Resolution order

For any setting: `--config <path>` > `<root>/knowledge.toml` > built-in defaults.

## 8. Migration from `.vaire/config.toml`

`vaire init` bootstraps or migrates in place:

- **Fresh package** → writes `<root>/knowledge.toml` (identity derived from the directory name,
  a starter `types` vocabulary) plus `.vaire/.gitignore`.
- **Legacy `.vaire/config.toml` present** → **migrates** it into `knowledge.toml`: renames
  `id_prefixes` → `types`, converts a non-empty `scoped_types` list into
  `scoped_types_whitelist` (preserving its "only these types" intent as the new lint policy;
  an empty list is dropped), drops `[embeddings]`, injects the required `name`/`version`, and
  carries `include`/`exclude`/`vocabulary_strict`/`scope_field` across. The old file is set
  aside as `.vaire/config.toml.migrated`. Comments are not preserved (the file is regenerated).
- **`knowledge.toml` already present** → `init` refuses to overwrite it (usage error), even if a
  legacy config sits alongside.
