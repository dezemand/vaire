# Vairë — the registry server: capability tiers

Status: **design, 2026-09-12. Tier 0 is live; tier 1 is next.**

[registry.md](registry.md) defines the distribution layer and its wire contract, and states
that a static file host is a full citizen: five paths, no server code. This document
defines what a **server** adds on top of that contract, and — more importantly — how it
adds it: as a ladder of capabilities that runtime configuration switches on one at a
time, each declared to clients through the descriptor the contract already has.

It replaces the server-first sketch of the v0.2 registry design (org-scoped endpoints,
one axum binary as the entry point) with the reverse ordering the catalog spec settled
on: the static layout is the floor, and a server is a static host that has learned
tricks.

Decisions recorded 2026-09-12: the server is Rust, deployed as one function per tier
(§5.2, §7); identity is OIDC with the **registry** naming the issuer and the CLI knowing
only the protocol (§2.3); pipelines publish by trusted publishing, never with a stored
secret (§2.3); authorization is expressed as roles the issuer assigns (§2.3); the
descriptor is a deploy artifact (§3); the begin/commit publish contract is specified in
full (§2.2).

## 1. Principles

- **The descriptor is the tier system.** `.well-known/vaire-registry.json` already
  declares `search`, `enumerable`, `publish`, `yank`, `validate_bump` and
  `access_enforcement`, and the client already degrades against whatever it finds
  (registry.md §10, the ladder). A server therefore never introduces a second notion of
  "what this registry can do". Its configuration decides which capabilities it
  implements, and **the descriptor is generated from that configuration** — never
  hand-written, never able to disagree with the routes actually mounted.
- **Tiers are strictly additive.** Each tier assumes every tier below it. A server
  cannot search what it did not ingest, cannot enforce access it cannot attribute to an
  identity, cannot validate a bump it did not verify. The ladder is a dependency order,
  not a feature menu.
- **The client never talks to storage.** A static registry lets the client `PUT` to a
  bucket; a server takes that over. From the publish tier up, the client authenticates
  to the *server*, and the server holds the storage credentials. No cloud-provider
  signing in the CLI, ever; `publish: put` stays for directories and lab hosts.
- **The CLI speaks one protocol; the registry names the party.** `vaire` knows how to
  run a standard OIDC login and how to send a bearer token. Which issuer, which tenant,
  which client, which scopes — the registry declares those. An identity provider is a
  deployment fact of one registry, never something the CLI has heard of.
- **Indistinguishable at the floor.** A server at the read tier serves the five paths
  byte-exact with ETags and nothing else. A client cannot tell it from a bucket, and must
  not be able to — `StaticHttp` works against it unchanged, which is what makes the read
  tier deployable with zero client work.
- **The shipped index is a claim** (registry.md §5.1). The server answers nothing from
  an index it did not rebuild itself. Everything a server *serves* — search hits, entity
  bodies, bump validation — comes from its own verified build of the shipped Markdown;
  `pull` alone returns the publisher's bytes verbatim, hash-pinned.
- **One core, several deployments.** The handlers are one crate. Locally they run as a
  single process (`vaire serve`, over a directory); on AWS the same route groups deploy
  as separate functions. Tiering is how the deployment is sliced, so each tier can be
  stood up, evaluated, and rolled back on its own.

## 2. The ladder

| tier | capability declared | what it requires | client change |
|---|---|---|---|
| **0 read** | `enumerable: true`, `publish` absent | serve the five wire paths from storage | none |
| **1 publish** | `publish: api`, `yank: true`, `auth` block | an ingest endpoint; server-held storage credentials; **identity on every write** | an `Api` publish path selected by the descriptor; `vaire registry login` |
| **2 enforce** | `access_enforcement: enforced` | roles applied to reads; restricted → 403 with hint, private → 404 | none — the token is already there |
| **3 verify** | `validate_bump: true` | unpack, rebuild, diff against the shipped index, check the claimed bump | none — refusals arrive as the existing typed errors |
| **4 search** | `search: lexical` | server-side state: verified indexes, queryable | `Registry::search`, the unwired seam |
| **5 query** | `query: read` (§8) | `resolve`/`render`/`backlinks`/`search` over ingested content, no install | the MCP-over-HTTP transport |

Tier 0 is what `file://` already is, and what `vaire serve` will be locally: the same
core over a directory instead of a bucket. Tier 5 is the reason the ladder exists at all —
it is the surface a tool-calling client with no local install reads an organisation's
knowledge through.

Identity is not a tier of its own. **Writes are never anonymous**, so validating a token
is part of the publish tier; what tier 2 adds is applying identity to *reads* and turning
the access flags from signals into controls.

### 2.1 Tier 0 — read

Five paths, from storage, with the ETag storage assigns. `packages.json` is served if
present and the descriptor says `enumerable`. Nothing is rewritten, compressed
differently, or resolved: a client caches index documents by ETag and a server that
re-encoded them would break every 304.

This tier has **no server-specific code**. On a bucket behind a proxy it is objects plus
a passthrough; on a directory it is a static file server. The descriptor here is the
same one a static push would have written.

Live since 2026-09-12 for one package, through the existing docs proxy.

### 2.2 Tier 1 — publish

The server becomes the only writer of `v1/*`. The publisher's contract is unchanged in
what it *guarantees* — a `(name, version)` is immutable once published, two publishers
cannot lose each other's work, an interrupted publish can be finished — and changed only
in *who executes it*: the server performs the create-only artifact write and the
compare-and-swap on the index, against storage it alone may write.

Every request in this tier carries a bearer token (§2.3) and needs the publish
permission for the package named. There is no anonymous publish, not even inside a
network perimeter.

#### 2.2.1 The begin/commit contract

Publishing is **two requests, not one**, because an artifact is larger than a request
body should be (and, on the intended AWS deployment, larger than the proxy allows — §6).
All bodies are JSON; all responses set `Content-Type: application/json` except where
noted. Every endpoint requires `Authorization: Bearer <token>` — a missing or invalid
token answers 401 with the challenge of §2.3, before anything else is checked.

**`POST /v1/api/publish/begin`**

Request:

```json
{
  "name": "togaf",
  "version": "10.0.1",
  "sha256": "4344cc5845a8…",
  "size": 14126567,
  "deps": { "acme-glossary": "^1" },
  "description": "…",
  "changelog_excerpt": null,
  "changelog": "## 10.0.1\n\n…",
  "access": { "listed": true, "pullable": true, "hint": null },
  "claimed_bump": "patch",
  "prior_version": "10.0.0"
}
```

Every field the index entry will eventually need travels here, exactly as
`PublishRequest` already carries them client-side (registry.md §9, `vaire/src/registry/
mod.rs`) — begin is that struct made a wire document.

Checks, in order, each a distinct failure the client can act on:

1. **name** — `checked_name` (registry.md §10.2): malformed names never reach storage.
2. **permission** — the caller's roles must include publish for this package (§2.3).
3. **already published** — the index already lists `(name, version)` → `409
   VersionExists`. This is not a race check; it is the ordinary "nothing to do" case a
   retried CI job hits constantly, and it is answered without touching staging at all.
4. **size** — over the deployment's configured ceiling → `413 Too Large`. The ceiling is
   an operational limit, not part of the wire contract; a deployment declares its own.

Response, `200`:

```json
{
  "upload": {
    "method": "PUT",
    "url": "https://…/staging/togaf/10.0.1/9f2e…/artifact",
    "headers": { "content-type": "application/gzip" },
    "expires_at": "2026-09-12T20:00:00Z"
  },
  "publish_token": "b64…"
}
```

`upload` is a single-use location outside the wire layout (§2.2.2) — a presigned object-
store URL on AWS, a plain authenticated endpoint on a directory-backed server. The client
`PUT`s the artifact there directly, with no further involvement from `begin`. `expires_at`
bounds how long the location is valid; a client that misses the window calls `begin`
again — idempotently, since step 3 above has not changed.

`publish_token` is an opaque, server-issued reference to this exact begin call (the
declared coordinates, the staging location, and an expiry) — not a second bearer token.
It is what lets `commit` know which upload it is completing without re-deriving anything
from the client's say-so; the client's own identity is still asserted on `commit` by the
same `Authorization` header as always.

Errors reuse the registry's typed vocabulary so the CLI's existing fan-out dispositions
apply unchanged, with one addition: `PermissionDenied`, a **new** `RegistryError` variant
whose disposition is `Fatal`. `Unreachable`'s own disposition is `Partial` — a network
timeout is exactly the case where a fan-out reports what it has and moves on — which is
the wrong shape for "authenticated, but not permitted", a fact about the caller that
retrying elsewhere cannot change. Reusing `Unreachable` for both would make a fan-out
treat a permission refusal as a transient network blip.

| status | body `error` | meaning | CLI disposition |
|---|---|---|---|
| 400 | `Malformed` | name or a field failed validation | fatal |
| 401 | — (`WWW-Authenticate` only) | no or expired token | login, then retry |
| 403 | `PermissionDenied` | authenticated, but lacks publish permission | fatal, names the permission |
| 409 | `VersionExists` | already published | not an error in `push`'s ordinary flow |
| 413 | `Io` | artifact exceeds the deployment's ceiling | fatal |

**Upload** — the client `PUT`s the artifact bytes to `upload.url` with `upload.headers`.
This request is *not* part of the wire contract proper: it is answered by whatever the
storage backend's presigned-upload mechanism answers (a plain `200`/`204` from an object
store, or the directory-backed test server's own ack). The one contract it keeps
regardless of backend, whichever kind of location `upload.url` turns out to be: it grants
**exactly one expiring `PUT` to the one object `begin` staged**, nothing else — no listing
of `staging/`, no reading or deleting any object under it, and no access to any other
package's staging area. An object store's presigned URL gets this for free (it is scoped
to one key by construction); a plain authenticated endpoint on a directory-backed server
has to enforce it deliberately, by checking the token names an upload *this* request is
allowed to complete rather than accepting any valid bearer token for any staged path. A
checksum mismatch is still caught at `commit`, never trusted here — that contract is
unrelated to and does not substitute for scoping the upload capability itself.

**`POST /v1/api/publish/commit`**

Request:

```json
{ "publish_token": "b64…" }
```

Nothing else — every fact about the release was already asserted at `begin` and is
looked up from the token server-side. This is deliberate: a client cannot smuggle a
changed version or a different package into commit by editing this request, because
commit has nothing to edit.

The server, in order:

1. Reads the staged bytes and checks their length against the `size` declared at
   `begin`, then their digest against the declared `sha256`. Either mismatch is `422
   ChecksumMismatch` and stops here — nothing downstream ever sees unverified bytes. The
   length check is not redundant with the digest one: `begin`'s size ceiling (§2.2.1)
   is enforced against the *declared* size, and a publisher who declares a small size to
   pass that check, then uploads something larger and reports *that* upload's real
   digest, would otherwise have their true size go unchecked all the way through — the
   digest matches because it was computed honestly for the oversized bytes, and only
   comparing the staged length against what was declared at `begin` catches the lie the
   ceiling was supposed to stop.
2. Runs the publish choreography of registry.md §9.2 exactly as a static host's
   conditional writes would, with the server itself performing each write: create-only
   copy from staging to `v1/artifacts/<name>/<name>-<version>.tgz`, write the changelog,
   compare-and-swap `v1/index/<name>.json`, best-effort merge into `v1/packages.json`.
3. Deletes the staged object.

Response, `200`:

```json
{
  "name": "togaf",
  "version": "10.0.1",
  "sha256": "4344cc5845a8…",
  "size": 14126567,
  "artifact_url": "https://…/v1/artifacts/togaf/togaf-10.0.1.tgz",
  "warnings": []
}
```

The exact shape of `Published` (registry.md §9) — so a client's success handling is
identical whether it published via `put` or `api`.

| status | body `error` | meaning | CLI disposition |
|---|---|---|---|
| 401 | — | token expired between begin and commit | login, then `begin` again (the staged upload survives) |
| 404 | `NotFound` | unknown or expired `publish_token` | `begin` again |
| 409 | `VersionExists` | another commit finished first (the push race, registry.md §3.3) | refresh, recompute the bump, retry — unchanged from the static-host case |
| 422 | `ChecksumMismatch` | staged bytes do not match the declared digest | fatal; re-`begin` with the real bytes |

**The half-done-publish rule survives intact.** If commit is interrupted between the
artifact write and the index write, a retried `commit` with the *same* `publish_token*
finds the artifact already in place, byte-identical, with no index entry — exactly the
condition registry.md §9.2 already names as this push's own earlier attempt — and
finishes the job rather than refusing. A `publish_token` that has expired forces a fresh
`begin`, which is safe: step 3 of begin's checks (already published) catches the case
where commit actually succeeded before the client's connection dropped.

**`POST /v1/api/publish/yank`**

Request: `{ "name": "togaf", "version": "10.0.1", "yanked": true }`. Same permission as
publish for the package. Edits the index document only — the artifact never moves,
exactly as registry.md §9.2 specifies. Response `200`, empty body.

#### 2.2.2 The staging area

Not part of the wire layout (registry.md §9) and never served by the read tier — a
client with a valid artifact `sha256` cannot address staged bytes by guessing a path,
because the location `begin` returns is single-use and keyed by an opaque identifier, not
by `(name, version)`.

- **Naming**: `staging/<name>/<version>/<random>/artifact` — the random component is
  what makes two concurrent `begin` calls for the same version (a retry racing an
  original attempt) land at different objects, so neither can be corrupted by the other
  finishing first. Only `commit` ever reconciles them, by trusting exactly one
  `publish_token`.
- **Lifecycle**: written once by the client's upload, read once by `commit`, deleted by
  `commit` on success. A staged object whose `publish_token` never commits is swept by a
  scheduled job past its `expires_at` — cheap, because nothing else in the system ever
  depends on a staged object outliving its token.
- **Permissions**: the publish identity may read and delete under `staging/`; nothing
  else may. This is symmetric with `v1/*` being writable only by the same identity — the
  two prefixes are the entire write surface a publish credential has.

### 2.3 Identity

The registry validates tokens; it never issues or stores credentials. Every token is an
OIDC-issued JWT, checked against the issuer's published signing keys (fetched from the
issuer's discovery document and cached per process), the issuer as `iss`, and the
registry's declared audience as `aud`.

**HTTPS is mandatory for any registry that takes a token** — one whose descriptor
carries an `auth` block, or declares `publish: api`, whose writes are never anonymous
(§2.2) whether or not an issuer is advertised — with one documented exception:
`http://127.0.0.1` and `http://localhost`, for `vaire serve` run against a local test
issuer with nothing to eavesdrop on. A bearer token is a bare credential — anyone who
reads the wire reads the identity it names — so such a registry over plain `http://` is
refused at `registry add`, the same boundary that already validates a URL before storing
it; and the client refuses to attach a token in clear even for a row recorded before the
rule existed. The same standard applies to where `begin` sends the upload (§2.2.2): the
client accepts an `https://` destination, or one on the registry's own origin, and
nothing else — a `begin` response pointing elsewhere is malformed, not followed.

#### Discovery: the descriptor names the party

An optional `auth` block in the descriptor declares what a **person** should log in
against: the OIDC issuer, the public client id the CLI presents, the scopes to request,
and which grants the issuer supports. From the issuer, standard OIDC discovery gives the
CLI every endpoint it needs. Absent block means anonymous, which is what every existing
descriptor already implies — adding it is not a schema break.

An unauthenticated request to something that needs identity answers **401 with a
`WWW-Authenticate: Bearer` challenge** naming the realm and the scope it wanted, so the
CLI can say exactly what to do ("run `vaire registry login central`") instead of guessing.

#### People: `vaire registry login`

The **device authorization grant** (RFC 8628) is the baseline: the CLI prints a code and
a URL, the person signs in with their ordinary account in any browser, the CLI polls and
receives tokens. It works over SSH, in containers, and on every issuer of note.
Authorization code with PKCE on a localhost redirect is a better laptop experience and is
offered when the descriptor lists it — device code is the one that always works.

Tokens land in `credentials.toml`, keyed by registry name — the file's documented purpose
since 0.2. A refresh token makes the login a one-time event; a 401 for an expired token
names the command to run. `vaire registry logout` forgets; `vaire registry show` reports
whether a login is on file. Operating-system keychain storage is a later refinement, not
a design question.

#### Pipelines: trusted publishing, no secrets

A CI system mints an **OIDC ID token per job** whose claims say which project and ref is
running. That token *is* the pipeline's identity, and the registry accepts it directly:
its configuration lists the CI issuer as trusted, with an audience and **claim
constraints** binding projects to packages — "a token from `gitlab.scania.com` whose
`project_path` is `sflse/kg/togaf` may publish `togaf`". This is the model npm, PyPI and
crates.io converged on. Nothing is stored anywhere, and the binding "who may publish this
package" lives in the registry's own configuration, which is where it belongs.

The CLI is identical in a pipeline: it sends whatever bearer token the environment gives
it. `VAIRE_TOKEN` (and a per-registry form, so two registries can coexist in one job)
takes precedence over `credentials.toml`; `vaire push` in a job needs no login step.

Exchanging the CI token at the organisation's identity provider for one of *its* tokens
was considered and not chosen: it makes every publishing pipeline an object to administer
in the identity provider, and moves the package↔project binding away from the registry.
A deployment that wants a single issuer can still configure exactly that.

#### The trusted-issuer list

Server configuration is a **list of trusted issuers**, each with its audience and its
claim rules. An organisation's identity provider for people and its CI system for
pipelines is two entries; a different organisation lists different ones with the same
code. The `auth` block in the descriptor advertises only the issuer people log in
against — the CI entry is server-side, because nothing interactive ever needs it.

#### Authorization

The CLI never interprets claims. The registry maps them to permissions:

- for tokens from the people-issuer, **roles the issuer assigns** — application roles
  such as `Registry.Publish` and `Registry.Read`, granted to users, groups or service
  principals where the organisation already manages membership. The functions read a
  `roles` claim; they never learn a group id.
- for tokens from a CI issuer, the project↔package bindings above.

The access flags of registry.md §9.3 resolve against the resulting permissions: a
restricted package answers 403 carrying its hint verbatim (existence should leak — that
is the point), a package the caller may not know about answers 404 on every path.

Read access is a per-registry policy switch. Inside a network perimeter a registry may
stay anonymous-readable and demand identity only to write — the pilot's posture. The
perimeter is not identity, though, and the moment anything is `restricted` reads need a
token too; that is tier 2.

#### The validator seam

Nothing above names a specific identity provider in code. The server holds a list of
issuer entries — issuer URL, audience, and (for CI issuers) claim constraints — behind
one `TokenValidator` interface: given a bearer token, answer the caller's identity and
roles, or refuse. A real OIDC validator (JWKS fetch and cache, standard claim checks) and
a fixed-token validator (a static map of token → identity, for tests and for `vaire
serve` run locally with no IdP in reach) implement the same interface, so which one a
deployment runs is configuration, not a code fork. Wiring an organisation's actual issuer
(Entra or otherwise) in is filling in this seam's configuration, not building it.

### 2.4 Tier 2 — enforce

With identity already validated at tier 1, this tier is small: apply the permissions to
reads, and flip the descriptor to `access_enforcement: enforced` — not before, because
claiming enforcement that is not there is the failure mode with consequences.

### 2.5 Tier 3 — verify

Commit grows a step between checksum and choreography: unpack with containment (every
entry strictly under the target; links and absolute paths refused; under `.vaire/` only
the index accepted), confirm the manifest's declared name and version match the upload
coordinates, **rebuild the index from the shipped Markdown with the same crate the CLI
uses, and diff it against the shipped one**. A mismatch rejects the publish. This is the
server's half of "the index is a claim": the only thing that crosses into what the server
will later *answer from* is what it built itself.

The bytes being unpacked and parsed here passed the checksum in §2.2's commit step, not
a trust check — a publisher able to reach `begin` can shape the archive's *contents*
however it likes, so this is the one tier that unpacks and parses attacker-controlled
input, and it is treated that way, not just in the containment check above but in what
the deployment is willing to spend on it: an expanded-size ceiling (independent of the
already-checked compressed artifact size — a small archive can still expand large), an
entry-count ceiling, a maximum extraction depth, and a wall-clock timeout on the whole
verify step, past which it is refused rather than left running. These are deployment
operational limits, the same kind of number as §2.2.1's artifact size ceiling — not part
of the wire contract, and not designed here; a deployment sets them to what its own
compute budget can absorb.

Once the rebuilt index exists, `validate_bump` is nearly free: diff the entity set of the
prior release's verified index against this one, classify exactly as `vaire release`
does, and refuse a claimed bump the diff does not support — a PATCH that removed an
entity, a MINOR that renamed one. The refusal names what it found.

### 2.6 Tier 4 — search

The server holds every release's **verified** index and can answer a lexical query across
them. Two shapes were considered:

- **federated** — one index per release, exactly as verification left it, fanned out per
  query and merged by tier and score. This is the `federated-index` decision applied to
  the server: nothing is ever merged into a single database, a release's index is
  immutable alongside its artifact, and a yanked or restricted package is simply skipped
  at fan-out time.
- **merged** — one registry-wide index rebuilt at ingest. Faster per query, and a second
  representation of the same content that has to be kept honest.

This document chooses **federated**, for the same reasons the corpus design did, and
accepts the cost: query latency grows with package count. At the dozen packages the
pilot registry serves that is not a constraint; the shape is revisited when it is.

Only the *current* release per major line is searched by default. Search answers
"where is this knowledge now?", and a hit in a superseded patch is noise.

### 2.7 Tier 5 — query

The read commands, over content nobody installed. This is the tier the milestone is
for: a client with no checkout, no manifest and no `vaire` binary asks a question, and an
organisation's packages answer it — with attribution, because every hit names the package
and release it came from.

The surface is the read tool set the MCP server already exposes (`resolve`, `render`,
`backlinks`, `refs`, `search`, `suggest`, `deps`), served over HTTP, scoped to what the
caller's identity may see. It is *not* a new query engine: the same commands, over the
verified indexes of tier 4, resolved through the same code paths as the CLI. Two
transports carry it — MCP-over-HTTP for MCP-speaking clients, and a plain REST/OpenAPI
form for tool-calling clients that speak neither — and neither gets anything the other
lacks.

`render` at this tier needs the shipped Markdown, not only the index. The artifact has it;
the server unpacks on demand and caches. Attachments are served by their
package-relative path from the same unpack — and because that path and its bytes are
entirely a publisher's choice, an attachment response never trusts what a publisher named
it: `Content-Type` is inferred from a fixed extension allowlist rather than taken from
anything in the artifact, every response carries `X-Content-Type-Options: nosniff` and a
`Content-Disposition` that forces a download for any type outside a small inline-safe set
(images, PDF, plain text), and the registry's own origin never executes what it serves —
no attachment is rendered inline in a context that could run script from it. A knowledge
package is content agents and people read (registry.md §13's "content is a source, not an
authority"); the same caution applies to what a browser is asked to do with the bytes.

Scope defaults follow the existing rule: a query answers from the registry it was asked
of. Fan-out across registries is the *client's* catalog job, not the server's.

## 3. Configuration is the descriptor's source

A server is configured with three things:

- **storage** — a directory, or a bucket and the role that may write it;
- **the tier**, or equivalently the set of capabilities to mount;
- **identity** — the trusted-issuer list (§2.3), which of them people log in against,
  and whether anonymous read is allowed.

The descriptor is rendered from this, and every route is mounted or refused from the
same source, so a capability is either fully present (declared *and* served) or fully
absent. A server may not declare a capability it has not been configured to serve; there
is no "declared but 501" state. This is what lets a client trust the ladder.

Where the tiers are separate deployments (§5.2), no single process knows which of them
exist — so **the descriptor is written by the deployment**, from the set of tiers it
deployed, and is an object in storage like every other wire document. Locally, where one
process serves every tier it runs, the process writes it at startup. Either way it is
generated, and either way the rule holds.

The name `vaire serve` keeps its catalog-spec meaning locally — the same core over a
directory, read tier, idle-exit — and gains flags only for the tiers a local instance can
sensibly run (publish over a directory is one; verification is another; identity on
localhost is a test fixture).

## 4. What tiering changes for the client

Very little, and all of it selected by the descriptor:

- `publish: api` routes `push` and `yank` through begin/commit instead of conditional
  `PUT`s. `publish: put` keeps today's transport. The `Registry` trait does not change;
  `StaticHttp` grows an alternative publish path, or a sibling `Api` implementation
  shares its reads.
- An `auth` block enables `vaire registry login <name>` / `logout`, and `registry show`
  reports the login state. Without the block the verbs refuse with "this registry is
  anonymous".
- A bearer token is resolved **per registry, never globally**: `VAIRE_TOKEN` is read only
  when exactly one registry is the target of the current command (`push --registry
  central`, or the one registry `select` would already pick unambiguously); its
  per-registry form (`VAIRE_TOKEN_<NAME>`) or `credentials.toml` is what a command
  touching several registries in one fan-out uses instead, so a token never travels to a
  registry the caller did not name. Before sending it, the client confirms the token's own
  `iss`/`aud` match what that registry's descriptor declared — a token minted for one
  registry is not attached to a different one just because both happen to be configured.
  Nothing about tokens reaches a manifest or a lockfile, and a redirect response is never
  followed with the `Authorization` header attached unless the redirect target is the same
  origin the token was resolved for.
- `search: lexical` lets the fan-out engine use `Registry::search` instead of walking the
  degradation ladder to enumeration.
- Enforced access surfaces as the errors that already exist — `PullRestricted` with a
  hint, the new `PermissionDenied` for a 403 that is not a restriction, `NotFound` for a
  404 — plus one new one for a 401, which carries the login command.

Everything above the trait — the catalog, `pull`, resolution, the lockfile — is untouched.

## 5. Deployment shapes

### 5.1 Local — `vaire serve`

One process over a directory. Tier 0 by default; the same directory a `file://` registry
uses, so anything published to it by conditional `PUT` and anything published through
the server's begin/commit land in one layout. The integration suite runs the ladder over
this: the same conformance tests that prove `file://` prove the server at every tier it
can run locally, with the fixed-token validator (§2.3) standing in for identity.

### 5.2 AWS — one Rust function per tier

The handlers are one crate, `vaire-registry`, depending on `vaire` as a library, with the
routes split into groups: read, publish, search, query. Storage sits behind a trait with
a directory and an object-store implementation — the server-side twin of the client's
`Transport` seam. Each deployed function is a thin binary that mounts one route group and
hands it to the function runtime; the same router on a plain listener is `vaire serve`.
The handlers do not know which of the two they are behind.

The existing host rule on the shared ALB fronts everything, routing by path to each
function's target group:

| route group | function | storage access |
|---|---|---|
| `/.well-known/…`, `/v1/index/…`, `/v1/artifacts/…`, `/v1/changelogs/…`, `/v1/packages.json` | the existing docs handler — already passes these through byte-exact | read |
| `/v1/api/publish/…` | publish | write to `v1/*` and `staging/*` — the **only** principal allowed to |
| `/v1/api/search`, `/v1/api/query/…`, `/mcp` | search & query | read, plus the verified-index store |

Identity is not a function; it is a layer each function applies, with the trusted-issuer
list as deployment configuration. The publish function is where the CI docs-publish
role's deliberate confinement to `docs/*` finally pays off: `v1/*` has exactly one
writer, and it is not a pipeline.

Functions are built with `cargo-lambda` (cross-compiles from any host) or in the Linux
image `vaire-ci` maintains, and deployed by `vaire-aws` as asset-based functions — one
construct per tier, so a tier ships as one deployment diff. The descriptor is written by
that deployment (§3).

Rust on the function runtime starts in tens of milliseconds, which is what makes the
per-request fan-out of search and query the cost that matters rather than startup; and
the verify tier rebuilds an index in the function's ephemeral disk with the `vaire`
crate, unchanged.

## 6. Constraints the AWS shape imposes

- **Request and response bodies are capped at 1 MB** between the ALB and a function, and
  binary bodies are base64-encoded first. This is why publish is begin/commit with a
  direct upload (§2.2) and why `pull` of a large artifact is a redirect to a short-lived
  signed URL, exactly as the docs handler already does for large images. Index documents,
  changelogs and descriptors are small and are served inline.
- **Cold starts** decide what search and query may load per request. A verified index is
  loaded from storage into the function's local disk on first use and reused for the
  container's lifetime; the federated shape (§2.6) keeps each load small.
- **The perimeter is network, not identity.** Everything reaching a function is already
  inside the WAF's IP allowlist. That is sufficient for anonymous read at tier 0 and
  insufficient for anything a tier above it promises — which is why writes carry a token
  from the first tier that has any.
- **Storage is shared with the docs site.** One bucket, split by prefix; the registry
  prefixes are the contract from registry.md §9 (plus `staging/*`, §2.2.2) and the docs
  prefixes are the renderer's. Neither writer may cross into the other's.

## 7. Language

Tiers 0–2 could be written in anything. Tier 3 rebuilds an index from Markdown with the
CLI's own parser, and tiers 4–5 run the CLI's own read commands; reimplementing those is
a second corpus semantics that would drift from the first. So the server is **Rust,
reusing the `vaire` crate** — from the first tier that has code, not from the first tier
that strictly needs it, because a publish flow is not worth writing twice. The existing
TypeScript read handler stays as it is.

## 8. The query capability

`query` is a sixth descriptor field, absent today: `query: none | read`. It is separate
from `search` because a registry can sensibly search titles without serving entity
bodies, and because the two have different access consequences — search reveals that
something exists, query reveals what it says. Declaring it is what tells a client (and a
catalog's reader mode) that this registry can be *read from* rather than only *pulled
from*.

Adding a field is not a schema break: absent means none, which is what every existing
descriptor already implies.

## 9. Open

- **URL shape for the API groups.** `/v1/api/…` beside the static paths, flat names, no
  organisation segment — until a second organisation exists. The v0.2 sketch's `/org/:org`
  is not adopted.
- **Token storage** — `credentials.toml` first; an operating-system keychain when a
  platform makes plaintext-at-rest unacceptable.
- **Retention of staged uploads**, and of unpacked content the query tier caches.
- **Search over yanked releases** — excluded by default; whether an exact-version query
  may still reach one.
- **Rate and size limits at ingest**, beyond the size ceiling in §2.2.1. Verification is
  the one place the server treats input as hostile, and it runs a parser over it; caps
  belong there before anything is exposed beyond the perimeter.
- **Wiring the organisation's real OIDC issuer** into the validator seam of §2.3 — the
  seam is designed and the fixed-token validator proves the contract; a live IdP is
  configuration, done when a deployment target is chosen.

## 10. Build order

Each step is evaluable without the next:

0. **Read tier on AWS.** Descriptor plus one package's index and artifact in the bucket;
   `vaire registry add` and `vaire pull` from a laptop through the proxy. **Done
   2026-09-12**, with the client as it was.
1. **Publish** — the begin/commit contract (§2.2, done); `vaire serve` running it over a
   directory against the conformance suite; the client's `Api` path; the publish
   function on AWS with token validation and the trusted-issuer list; CI publishing on
   tag by trusted publishing.
2. **Enforce** — permissions applied to reads; enforced access flags.
3. **Verify** — rebuild-and-diff at commit, then `validate_bump`.
4. **Search** — lexical, federated over verified indexes.
5. **Query** — the read commands over HTTP, then MCP-over-HTTP in front of them. This is
   the step at which a tool-calling client with no install can hold a conversation
   grounded in the organisation's packages, which is what the line was for.
