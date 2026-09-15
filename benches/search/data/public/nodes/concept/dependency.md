---
id: dependency
type: concept
name: Dependency
---
# Dependency

A dependency is two separate facts about one other [[concept:package]]: **what** you
depend on, declared in the committed `[dependencies]` table as `name = "^MAJOR"`; and
**where** it lives on this particular machine, which is per-checkout state under
`.vaire/packages/<name>` and never committed (see [[concept:linked-package]]). Keeping
these apart is what lets the same manifest work unmodified on every machine that clones it.

`^MAJOR` is the only constraint form Vairë accepts — no tighter pins, no ranges — because
minor and patch changes never change meaning by definition (see
[[decision:caret-major-only]]), so a dependent simply rides a major line and adopts
everything under it automatically. `vaire add <name>[@^N]` declares it; `vaire check` flags
an undeclared one used in a reference ([[finding:undeclared-import]]) and a declared one
nobody ever references ([[finding:unused-dependency]]).
