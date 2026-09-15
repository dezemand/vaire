---
id: undeclared-import
type: finding
name: Undeclared import
aliases: [undeclared_import]
---
# Undeclared import

Fires when a prose or frontmatter reference names a package with `@pkg/type:id` syntax, but
that package alias never appears in the current [[concept:manifest|manifest's]]
`[dependencies]` table. It's a pure table lookup — nothing about whether the target package
is even reachable matters yet, only whether it was ever declared.

If the reference itself is correct, the fix is to declare it in the same change that
introduces the reference: `vaire add <pkg>`. If it isn't, either fix the intended package
name or demote the reference to a [[concept:loose-end]] instead — and never guess a package
qualifier onto a loose end, since a descriptor's package is unknown by definition until
someone resolves it. This finding is a violation, distinct from
[[finding:missing-dependency]], which fires when a package *is* declared but can't actually
be located on this machine.
