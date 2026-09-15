---
id: registry
type: concept
name: Registry
---
# Registry

A registry is a remote source this machine publishes [[concept:artifact|artifacts]] to and
pulls them from, configured with `vaire registry add <name> <url>`. The client tells a
static file host apart from a full API server only by declared capabilities — see
[[decision:static-host-full-citizen]] — so a plain directory, an S3 bucket, or anything
that can serve five well-known JSON paths and a handful of tarballs already implements the
whole protocol.

Registering one is a decision, never an observation the way a [[concept:catalog]] sighting
is: nothing ambient ever writes a registry row, and nothing self-heals it either — it
exists because someone named it and leaves when someone removes it. `--priority` orders
fan-out and breaks ties when a command needs exactly one registry and wasn't told which;
resolution asks them in that order and the first one that satisfies wins outright, never
the highest version across all of them, because a registry is a trust boundary as much as
a location.
