---
id: malformed-diagram-ref
type: finding
name: Malformed diagram reference
aliases: [malformed_diagram_ref]
---
# Malformed diagram reference

A `vaire/` marker inside a diagram source — a fenced PlantUML or Mermaid block, or an
external `.puml`/`.mmd`/`.drawio` file a node's prose points at — whose target after the
prefix doesn't parse as a legal reference (`vaire/Team:Alpha`, `vaire/Not A Reference`). No
edge gets created, and before this finding existed such a typo had nowhere to surface at
all: not an edge, since there was no address to record, and not a
[[concept:loose-end]] either, since that form needs a literal space after `?` and a diagram
is deliberately not a place to record an open question.

The fix is applied directly in the diagram source, at the exact file and line the finding
names: correct the target to a real, lowercase `type:id`. There's no loose-end escape
hatch here the way there is for ordinary prose — if the entity doesn't exist yet, the
marker should point at something that does, or the link should simply wait until it does.
