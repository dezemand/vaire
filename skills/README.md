# Vairë skills

Agent skills for working with Vairë corpora, in two layers. **Reference skills** say how
things work — the mechanics, stated once. **Situational skills** say what to do in a
recognizable situation — workflow plus judgment — and cite the reference skills instead
of repeating them. Situations map onto the roles in `docs/guidelines.md`: what a skill
lets you do is bounded by what its role may change.

## Reference — how things work

| skill | covers |
|---|---|
| [vaire-files](vaire-files/SKILL.md) | the corpus file model: nodes, `type:id`, references, loose ends, frontmatter edges, scoped IDs |
| [vaire-packages](vaire-packages/SKILL.md) | the package model: `knowledge.toml`, dependencies, linking, cross-package resolution, version semantics |
| [vaire-query-cli](vaire-query-cli/SKILL.md) | the read/maintenance commands from a shell |
| [vaire-query-mcp](vaire-query-mcp/SKILL.md) | the same read surface over MCP |

## Situational — what to do when

| skill | the situation | role |
|---|---|---|
| [vaire-answering](vaire-answering/SKILL.md) | answering a question from the corpus | observer |
| [vaire-contributing](vaire-contributing/SKILL.md) | you learned something — records, additive prose, loose ends | contributor |
| [vaire-entity-authoring](vaire-entity-authoring/SKILL.md) | writing, converting, or reviewing an entity file | contributor/maintainer |
| [vaire-entity-creation](vaire-entity-creation/SKILL.md) | running the gated pass: loose ends → identities | maintainer |
| [vaire-package-curation](vaire-package-curation/SKILL.md) | package boundaries, the substance bar, the type ladder, migrations | maintainer |
| [vaire-versioning](vaire-versioning/SKILL.md) | deciding a bump, cutting a release, consuming a MAJOR | maintainer |
| [vaire-release-summary](vaire-release-summary/SKILL.md) | writing the prose `vaire release --summary` carries into the record | contributor |
| [vaire-check-triage](vaire-check-triage/SKILL.md) | `vaire check` failed — what each finding means and its fix | any |

An agent doing autonomous corpus work typically loads **vaire-contributing** (its role),
**vaire-files** (the mechanics), and one query skill — and pulls the others in when their
situation actually arises.
