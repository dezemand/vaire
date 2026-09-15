---
id: manifest
type: concept
name: Manifest
---
# Manifest

`knowledge.toml` is the one authored, committed configuration file a [[concept:package]]
has — everything else Vairë touches lives under the gitignored `.vaire/`. It declares
identity (`name`, `version`), the file scope that bounds the corpus (`include`/`exclude`
globs), the [[concept:type-vocabulary]] the package defines, scoping settings, and its
`[dependencies]` table.

Only `name` and `version` are required; a two-line manifest is already a valid package.
Machine- and consumer-local choices — most notably which embedding provider indexes the
corpus — are deliberately excluded, because a package must not dictate how whoever consumes
it builds their own index. `vaire init` scaffolds a fresh manifest, or migrates a legacy
`.vaire/config.toml` into one; `vaire add` is the only command meant to edit
`[dependencies]` afterward, and it preserves the file's formatting and comments rather than
regenerating it.
