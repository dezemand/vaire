# Example workspace — local cross-package resolution

Three packages referencing each other (cli.md §6.5, design.md §9): **acme-core**
(`team`, `person`) and **acme-web** (`service`) in a dependency cycle, **acme-shared**
(`site`) a leaf. `acme-web` also carries one deliberately dangling reference
(`wiki: "@acme-shared/wiki:home"`) so `vaire check` has something to catch, and a
cross-package tombstone (`service:legacy` → `@acme-core/team:platform`).

Each package's `knowledge.toml` already *declares* its dependencies — that part is
committed. Where those dependencies live is per-checkout state under gitignored `.vaire/`,
so that is the one thing to wire up here.

## Setup

```sh
cd examples/workspace/acme-web
vaire add acme-core   --link ../acme-core
vaire add acme-shared --link ../acme-shared
vaire index                                  # builds this package + the linked closure
```

`--link` names the sibling directories explicitly, which works with no configuration at
all. If you keep your packages under a local-packages root (cli.md §6.6), copying the
three directories there instead means `vaire index` alone wires and builds everything —
the dependencies are already declared, so nothing else is needed.

## The v0.2.0 acceptance transcript (issue #2)

```sh
vaire resolve  @acme-core/team:platform    # → the acme-core file
vaire refs     service:checkout            # → its @acme-core / @acme-shared edges
vaire backlinks @acme-core/team:platform   # → the services here that reference it
vaire check                                # → wiki:… dangling cross-package (error);
                                           #   team/person/site resolve clean;
                                           #   the acme-core↔acme-web cycle terminates
vaire deps                                 # → acme-core ^1, acme-shared ^1 (resolved)
```

Also worth trying: `vaire resolve service:legacy` (a tombstone that hops packages),
`vaire search platform` (dependencies are part of your knowledge), and
`vaire suggest "jane doe"` (suggestions from dependencies arrive pre-qualified).
