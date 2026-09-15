---
id: caret-major-only
type: decision
name: "`^MAJOR` is the only constraint form"
---
# `^MAJOR` is the only constraint form

A declared [[concept:dependency]] may only ever say `^1`, `^2`, and so on — never a tighter
pin, a range, or an exact version in `knowledge.toml`. The reasoning is definitional rather
than stylistic: minor and patch releases never change meaning (see the predicate in
[[decision:computed-bumps]]), so a tighter constraint could not do anything but generate
churn for no safety benefit.

Since v0.3 the constraint doubles as a **selector**, not merely an after-the-fact lint: the
[[concept:catalog]] picks whichever candidate package satisfies the stated major line by
comparing parsed version triples, and where several members of one dependency closure
constrain the same name, their demands are intersected rather than solved — one major line
resolves, and disjoint majors are reported as a conflict naming both declarers, because a
closure links exactly one directory per package name. Wanting an exact version anyway is
what [[?command: the pin command]] is for — a deliberate, consumer-side override recorded in
the [[concept:lockfile]], layered on top rather than replacing the constraint.
