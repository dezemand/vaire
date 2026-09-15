---
id: rootless-session
type: concept
name: Rootless session
aliases: [catalog-scoped session]
---
# Rootless session

Every ordinary session assumes an author standing inside one [[concept:package]], scoped
to that package's own dependency closure. A rootless session is the opposite: run a read
command from anywhere with no `knowledge.toml` above the working directory, and the scope
widens automatically to every package this machine's [[concept:catalog]] currently knows
about — `vaire mcp` started outside a package is how an agent gets pointed at the whole
machine rather than at one checkout.

Inside this scope, nothing is local: **every** result is package-qualified, and a bare
`type:id` is refused rather than reported missing, since `type:id` means "in this package"
and there is no "this package" to mean. The scope stops exactly at the catalog's own
membership, too — it does not expand into the dependency closures of the packages it
finds, so something nobody has catalogued directly cannot surface just because a *sibling*
package happens to link it. Maintenance commands never work this way; `init`, `index`, and
friends still require a real package to act on.
