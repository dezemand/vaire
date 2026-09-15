---
id: static-host-full-citizen
type: decision
name: A static file host is a full registry citizen
---
# A static file host is a full registry citizen

The [[concept:registry]] wire contract is specified as a file layout first, with any
server-side behavior treated as an optional, separately-declared capability layered on
top. That ordering means a plain directory, an S3 bucket, or any HTTP server that can
serve five well-known paths and a handful of tarballs already implements the *entire*
protocol — nothing about publishing or pulling requires bespoke server code to exist at
all.

The two guarantees a registry has to make — an immutable `(name, version)` pair, and no
lost writes when two publishers race — are properties of *how a write happens* rather than
policy a server enforces: a create-only `PUT` makes storage itself refuse a duplicate
version, and compare-and-swap on the index document turns a simultaneous publish into a
retry instead of a silently lost release. Capabilities like search or authentication are
declared, not assumed, so a static host can honestly say `search: none` and a client
degrades gracefully rather than failing.
