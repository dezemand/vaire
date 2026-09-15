---
id: registry-client
type: component
name: Registry client
---
# Registry client

Implements the [[concept:registry]] wire contract behind one trait, with a single
`StaticHttp` implementation carrying a pluggable transport underneath it — which is what
lets a `file://` target run the exact same protocol code path as a real HTTP endpoint,
turning the test suite into a genuine conformance suite rather than a mock standing in for
one. The trait is synchronous by design: there's no async runtime layered above the storage
engine's own bridge, and an async method on a trait object is awkward in exactly the way
this client needs to avoid for its multi-registry fan-out.

Every error the client can produce carries an explicit disposition rather than a bare
failure: `NotFound` moves on to the next registry and is simply forgotten, while
`PullRestricted` moves on too but is *remembered*, so if nothing else ends up serving the
package, the caller is told exactly where to go ask. An HTTP 403 is always treated as
*unreachable*, never as *not found* — reporting a permission refusal as a missing package
would send someone looking for entirely the wrong problem.
