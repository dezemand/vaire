//! Remote registries — the client seam (registry.v2.md §8–§9).
//!
//! A registry is *somewhere packages are published*. The [`Registry`] trait is the whole
//! point of this module: it keeps the catalog, `push`, and everything above them
//! indifferent to what kind of thing answers. Two implementations are foreseen — today's
//! [`StaticHttp`] (a bucket, a web root, or a plain directory over `file://`) and, when
//! RBAC arrives, an API service — and the migration between them is invisible above the
//! trait.
//!
//! ## The wire contract is a pile of files
//!
//! Nothing here needs a server (§8.1). A registry is five kinds of document under one
//! base URL:
//!
//! ```text
//! /.well-known/vaire-registry.json          descriptor: schema version + capabilities
//! /v1/packages.json                         [names] — the `enumerable` capability
//! /v1/index/<name>.json                     a package's release index
//! /v1/artifacts/<name>/<name>-<ver>.tgz     immutable once written
//! /v1/changelogs/<name>/<ver>.md            readable BEFORE pulling a major
//! ```
//!
//! Server *behaviors* are capabilities the descriptor declares, never assumptions: a
//! static host says `search: none` and the client degrades rather than failing. What a
//! static host does get for free is the two semantics that actually matter — immutability
//! (a create-only write cannot overwrite a published artifact) and race handling (a
//! compare-and-swap on the index document turns a lost update into a retry) — because both
//! are properties of the *write*, not of a server (§8.4).
//!
//! ## Why this trait is synchronous
//!
//! §9 sketches it with `async fn`. It is written sync, deliberately, and the sketch is the
//! thing that gives here rather than the codebase:
//!
//! * Vairë has no async runtime to speak of. The one that exists is a current-thread
//!   runtime owned *inside* `Index`, which `block_on`s at the Turso boundary so the rest of
//!   the crate can stay synchronous; HTTP is already blocking `ureq` for the same reason.
//!   An async trait here would either need a second runtime or push `.await` up through
//!   every command — a rewrite the registry client has no standing to demand.
//! * `async fn` in a trait is not dyn-compatible, and a `Box<dyn Registry>` is precisely
//!   what a fan-out over heterogeneous registries needs. Working around that means
//!   `async_trait`'s boxed futures, which buys allocation and a macro for no concurrency
//!   this crate can currently use.
//! * The concurrency §10 actually asks for — parallel fan-out with a per-remote time
//!   budget — is threads-and-channels shaped, not futures shaped, at a fan-out width of
//!   "how many registries has this user configured". Single digits.
//!
//! Recorded as amendment 24 in the spec.
//!
//! ## What is deliberately not here yet
//!
//! [`Registry::search`] exists on the trait with a default `Unsupported` body, because the
//! seam should be complete; the fan-out *engine* that calls it (§10 — tiering, dedupe, the
//! partial-results banner) arrives with the store, where local and remote hits have to be
//! ranked together. `fetch` likewise has no `pull` command above it yet: it is exercised by
//! the conformance suite and consumed when the store lands.

pub mod static_http;
pub mod transport;
pub mod wire;

use std::path::Path;

use crate::model::Version;

pub use static_http::StaticHttp;
pub use wire::{Access, Capabilities, Descriptor, PackageIndex, ReleaseMeta, SearchCapability};

pub type RegistryResult<T> = std::result::Result<T, RegistryError>;

/// The schema version this client writes and can read (§8.2). Gates **hard**: a registry
/// declaring a higher one is refused rather than misread, so a future format change cannot
/// be silently half-understood by an old client.
pub const SCHEMA_VERSION: u32 = 1;

/// A published artifact whose bytes have been checked against the digest the registry
/// published for them.
///
/// The type exists so that **an unverified artifact has no type to be installed from**
/// (§9). It is constructed in exactly one place — the tail of [`Registry::fetch`], after
/// the comparison — so "did anyone verify this?" is answered by the signature of whatever
/// is holding it rather than by reading the call site.
#[derive(Debug, Clone)]
pub struct VerifiedArtifact {
    pub name: String,
    pub version: Version,
    /// Where the bytes landed.
    pub path: std::path::PathBuf,
    /// The digest, which the published one and the received bytes agreed on.
    pub sha256: String,
    pub size: u64,
}

/// One package as the registry lists it (§9, the `enumerable` capability).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PackageSummary {
    pub name: String,
    /// The highest non-yanked version, or `None` for a package whose every release is
    /// yanked — which is a real state, and not the same as an absent package.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<Version>,
    pub releases: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Reported so a restricted package can be badged rather than silently dropped —
    /// discoverability is the entire purpose of listed-but-not-pullable (§8.5).
    pub access: Access,
}

/// One hit from a registry that can search itself.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub name: String,
    pub version: Version,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    /// The registry's own relevance number, passed through untouched. **Never normalized
    /// against a local score** (§10): they measure different things, and averaging them
    /// would invent a ranking neither backend claimed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

/// What `publish` is asked to put on the wire.
pub struct PublishRequest<'a> {
    pub name: &'a str,
    pub version: Version,
    /// The packed artifact to upload.
    pub artifact: &'a Path,
    /// The changelog document for this release — the release record rendered as Markdown
    /// (§8.1, amendment 20). A dependent reads this *before* fetching anything.
    pub changelog: Option<&'a str>,
    /// The MAJOR invalidated-assumptions summary, carried into the index document so
    /// deciding whether to adopt a major never requires a download.
    pub changelog_excerpt: Option<&'a str>,
    /// This release's declared dependencies, so transitive resolution never downloads an
    /// artifact to read a manifest (§8.3, decision 9).
    pub deps: std::collections::BTreeMap<String, String>,
    pub description: Option<&'a str>,
    /// Access for the (package, registry) pair. `None` leaves whatever the index document
    /// already says — access is sticky until changed (§8.5).
    pub access: Option<Access>,
    /// What the publisher claims this bump was, for a registry that can check the claim.
    /// Recorded here because `validate_bump` is a declared capability; static hosts ignore
    /// it.
    pub claimed_bump: Option<crate::model::Bump>,
    pub prior_version: Option<Version>,
}

/// The outcome of a successful publish.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Published {
    pub name: String,
    pub version: Version,
    pub sha256: String,
    pub size: u64,
    /// Where the artifact now lives, for the human reading the output.
    pub artifact_url: String,
}

/// A registry client.
///
/// Every method is fallible with [`RegistryError`] rather than the crate's own error type:
/// the variants are what a fan-out dispatches on ([`RegistryError::disposition`]), and
/// flattening them into a string at this layer would throw away the only thing that makes
/// asking several registries in a row tractable.
pub trait Registry {
    /// The name this registry is configured under, locally. Not part of the wire contract:
    /// two people may know one bucket by two names.
    fn name(&self) -> &str;

    /// The base URL.
    fn url(&self) -> &str;

    /// The root document, fetched once when the client was opened.
    fn descriptor(&self) -> &Descriptor;

    /// Every release of `name`, oldest first. Yanked releases are **included** and flagged:
    /// a yank is a recommendation not to adopt, and a client resolving a lockfile still has
    /// to be able to see the version it already depends on.
    fn versions(&self, name: &str) -> RegistryResult<Vec<ReleaseMeta>>;

    /// Download one release into `into`, verifying it against the published digest.
    ///
    /// The digest check is not optional and not the caller's job — that is what makes
    /// [`VerifiedArtifact`] mean something. A mismatch leaves nothing usable behind.
    fn fetch(&self, name: &str, version: Version, into: &Path) -> RegistryResult<VerifiedArtifact>;

    /// Every package this registry serves. Requires the `enumerable` capability.
    fn list(&self) -> RegistryResult<Vec<PackageSummary>>;

    /// Search, for a registry that can. The default answers `Unsupported`, which is the
    /// first rung of the degradation ladder (§10) rather than a failure: a caller walks
    /// down to `list` + client-side matching, and then to an exact-name `versions` probe.
    fn search(&self, _query: &str, _limit: usize) -> RegistryResult<Vec<SearchHit>> {
        Err(RegistryError::Unsupported {
            registry: None,
            op: "search",
        })
    }

    /// Publish one release.
    fn publish(&self, request: PublishRequest<'_>) -> RegistryResult<Published>;

    /// Flag or unflag a release as yanked. The artifact never moves (§8.4) — a yank is an
    /// edit to the index document, so a lockfile pinning that version keeps resolving.
    fn yank(&self, name: &str, version: Version, yanked: bool) -> RegistryResult<()>;
}

/// What a fan-out should do about an error (§9.1, decision 12).
///
/// The spec states this as a table; it lives here as code so the two cannot drift. Nothing
/// fans out yet — one registry is one registry — but `push` and `registry show` already
/// need "is this worth reporting, or is it just this registry saying no", which is the same
/// question one column narrower.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Ask the next registry; this one has nothing to say. Forget it happened.
    Continue,
    /// Ask the next registry, but **remember**: if nothing else satisfies the request, this
    /// is the answer to surface, hint and all.
    Remember,
    /// This registry cannot do the operation. Walk the degradation ladder (§10).
    Degrade,
    /// The registry did not answer in time. Report a partial result, never a failure.
    Partial,
    /// Stop. Nothing further will make this work.
    Fatal,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("{what} is not in registry '{registry}'")]
    NotFound { registry: String, what: String },

    /// Listed, but not pullable. **The hint is the feature** (§8.5): restricted-listed
    /// exists to route someone to the owner rather than to stonewall them, so the message
    /// carries whatever the registry said, verbatim.
    #[error("{name} is published in '{registry}' but not pullable{}", match hint {
        Some(hint) => format!(" — {hint}"),
        None => String::new(),
    })]
    PullRestricted {
        registry: String,
        name: String,
        hint: Option<String>,
    },

    /// A capability this registry does not declare. The name is optional because the
    /// trait's default `search` body has no registry in scope to blame — it is a blanket
    /// impl, not a method on any particular one.
    #[error("{} does not support {op}", match registry {
        Some(registry) => format!("registry '{registry}'"),
        None => "this registry".to_string(),
    })]
    Unsupported {
        registry: Option<String>,
        op: &'static str,
    },

    #[error("registry '{registry}' did not answer within {seconds}s")]
    Timeout { registry: String, seconds: u64 },

    #[error("registry '{registry}' at {url} could not be reached: {detail}")]
    Unreachable {
        registry: String,
        url: String,
        detail: String,
    },

    /// A published `(name, version)` is immutable, so re-publishing one is refused by the
    /// *storage*, not by a policy check that could be raced (§8.4). Not an error in the
    /// ordinary push flow — `push` skips what is already there — but a real one when the
    /// same version was built from different bytes.
    #[error("{name} {version} is already published in '{registry}'")]
    VersionExists {
        registry: String,
        name: String,
        version: Version,
    },

    #[error(
        "checksum mismatch for {name} {version}: registry published {expected}, downloaded \
         bytes hash to {actual} — nothing was installed"
    )]
    ChecksumMismatch {
        name: String,
        version: Version,
        expected: String,
        actual: String,
    },

    #[error(
        "registry '{registry}' speaks schema version {found}, and this vaire understands \
         {supported} — run `vaire upgrade`"
    )]
    SchemaTooNew {
        registry: String,
        found: u32,
        supported: u32,
    },

    /// A compare-and-swap lost: someone else wrote the document between the read and the
    /// write. The push race (§3.4), and **not** a user-visible error in the normal path —
    /// `publish` retries it internally and only surfaces it if the contention never clears.
    #[error("'{registry}' was written by someone else while publishing {what}; retry")]
    Conflict { registry: String, what: String },

    /// A document that is there but is not what it claims to be.
    #[error("registry '{registry}': {doc} is malformed ({detail})")]
    Malformed {
        registry: String,
        doc: String,
        detail: String,
    },

    #[error("registry '{registry}': {detail}")]
    Io { registry: String, detail: String },
}

impl RegistryError {
    /// How a fan-out should treat this error.
    pub fn disposition(&self) -> Disposition {
        match self {
            RegistryError::NotFound { .. } => Disposition::Continue,
            RegistryError::PullRestricted { .. } => Disposition::Remember,
            RegistryError::Unsupported { .. } => Disposition::Degrade,
            RegistryError::Timeout { .. } | RegistryError::Unreachable { .. } => {
                Disposition::Partial
            }
            RegistryError::VersionExists { .. }
            | RegistryError::ChecksumMismatch { .. }
            | RegistryError::SchemaTooNew { .. }
            | RegistryError::Conflict { .. }
            | RegistryError::Malformed { .. }
            | RegistryError::Io { .. } => Disposition::Fatal,
        }
    }
}

impl From<RegistryError> for crate::error::VaireError {
    fn from(e: RegistryError) -> crate::error::VaireError {
        crate::error::VaireError::Registry(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_carries_its_hint_verbatim() {
        let e = RegistryError::PullRestricted {
            registry: "central".into(),
            name: "acme-powertrain".into(),
            hint: Some("request via #team-powertrain-knowledge".into()),
        };
        assert!(
            e.to_string()
                .ends_with("request via #team-powertrain-knowledge")
        );
        // Remembered, not forgotten: if no other registry serves it, this is the answer.
        assert_eq!(e.disposition(), Disposition::Remember);
    }

    #[test]
    fn the_fan_out_table_matches_the_spec() {
        let registry = || "r".to_string();
        for (error, expected) in [
            (
                RegistryError::NotFound {
                    registry: registry(),
                    what: "acme-core".into(),
                },
                Disposition::Continue,
            ),
            (
                RegistryError::Unsupported {
                    registry: None,
                    op: "search",
                },
                Disposition::Degrade,
            ),
            (
                RegistryError::Timeout {
                    registry: registry(),
                    seconds: 30,
                },
                Disposition::Partial,
            ),
            (
                RegistryError::ChecksumMismatch {
                    name: "acme-core".into(),
                    version: Version::new(1, 0, 0),
                    expected: "a".into(),
                    actual: "b".into(),
                },
                Disposition::Fatal,
            ),
        ] {
            assert_eq!(error.disposition(), expected, "{error}");
        }
    }
}
