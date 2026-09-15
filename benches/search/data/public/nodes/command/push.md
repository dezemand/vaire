---
id: push
type: command
scope: cli:vaire
name: vaire push
---
# vaire push

`vaire push [version] [--registry name] [--access state] [--dry-run]` uploads released
versions to a [[concept:registry]] — deliberately split from
[[cli:vaire/command:release]], since cutting a version is a Git act and uploading is
idempotent plumbing that can be retried without re-running anything else. It enumerates
this package's own release tags and rebuilds each [[concept:artifact]] straight from that
tag's tree, which is exactly what lets a container that cloned the repository thirty
seconds ago publish a release it didn't personally cut.

Versions the registry already lists are skipped as a clean no-op; naming one explicitly
bypasses that skip and re-verifies it by digest instead, which is the tool to reach for
when a conflict is suspected. `--access open|restricted|unlisted` is sticky per
(package, registry) pair and, on any static host, purely advisory — the underlying bucket
boundary is the real enforcement.
