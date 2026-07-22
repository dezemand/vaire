---
id: hr
type: department
name: Human Resources
aliases: [HR, People Ops]
status: active
updated: 2026-06-15
---
# Human Resources

Owns onboarding and headcount.

A reference to this department renders its `name:` as the link text (design.md §6) —
the example below is fenced so the indexer does not treat it as a real edge:

```text
[[department:hr]]      ->  [Human Resources](./hr.md)   display from name:
[[department:hr|HR]]   ->  [HR](./hr.md)                piped text wins
```
