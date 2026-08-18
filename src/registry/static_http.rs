//! [`StaticHttp`] — the registry protocol over anything that stores blobs at paths
//! (registry.md §9).
//!
//! This is the *whole* registry implementation for 0.3: a bucket, a web root, or a plain
//! directory, with no server code anywhere. The two behaviors a registry actually has to
//! guarantee — a published version is immutable, and two publishers cannot lose each
//! other's work — come from the conditional writes in [`super::transport`], so what is left
//! here is document choreography.
//!
//! ## Publishing is an ordered sequence, and the order is load-bearing
//!
//! 1. `PUT` the artifact, **create-only**. If storage says it is already there, this
//!    version is published and we stop. That check is not advisory: it is the same atomic
//!    operation that wrote it, so there is no window in which two publishers both believe
//!    they are first.
//! 2. `PUT` the changelog. Also create-only, but a collision here is *not* fatal — a
//!    changelog is a document about a release, and a retried push that already wrote it
//!    should keep going.
//! 3. Compare-and-swap the index document. Only now does the release become visible.
//!
//! Reversed, that ordering would publish a release the index announces but nobody can
//! fetch. In this order the worst interruption leaves an artifact nothing points at —
//! invisible, harmless, and re-published byte-identically on the next attempt because
//! `pack` is deterministic.
//!
//! ## The descriptor is created, not demanded
//!
//! §8.4 describes publishing to a registry that exists. Nothing says how the first one
//! begins, and requiring a hand-authored `vaire-registry.json` before a directory can
//! receive its first package would make "start with a dumb file host" a lie. So a writable
//! transport with no descriptor gets one written on first publish, with the capabilities
//! this implementation actually has. Reading a registry never creates anything.

use std::path::Path;

use crate::model::Version;

use super::transport::{Transport, TransportError};
use super::wire::{
    AccessEnforcement, Capabilities, Descriptor, PackageIndex, PublishCapability, ReleaseMeta,
    SearchCapability, checked_name,
};
use super::{
    PackageSummary, PublishRequest, Published, Registry, RegistryError, RegistryResult,
    SCHEMA_VERSION, VerifiedArtifact,
};

/// How many times a lost compare-and-swap is retried before the contention is reported.
/// Publishing is rare and the loser's retry is cheap (re-read, re-merge, re-write), so this
/// is generous — reaching it means something is continuously rewriting the document, not
/// that two people pressed publish at once.
const CAS_ATTEMPTS: u32 = 8;

pub const DESCRIPTOR_PATH: &str = ".well-known/vaire-registry.json";
pub const PACKAGES_PATH: &str = "v1/packages.json";

pub struct StaticHttp {
    name: String,
    transport: Box<dyn Transport>,
    descriptor: Descriptor,
    /// Whether a descriptor document was actually found, as opposed to synthesized for a
    /// location that has never received a push. The difference matters to a human: an
    /// empty registry and a directory that is not a registry yet both serve nothing.
    initialized: bool,
}

impl StaticHttp {
    /// Open a registry at `url`, reading its descriptor.
    ///
    /// A missing descriptor is **not** an error: an empty directory is a valid
    /// not-yet-initialized registry, and refusing to open one would mean the first `push`
    /// could never happen. What such a registry gets is the capability set this client
    /// implements, which is also exactly what will be written to it on first publish.
    pub fn open(name: &str, url: &str) -> RegistryResult<StaticHttp> {
        let transport = super::transport::open(url).map_err(|e| translate(name, url, e))?;
        StaticHttp::with_transport(name, transport)
    }

    pub fn with_transport(name: &str, transport: Box<dyn Transport>) -> RegistryResult<StaticHttp> {
        let url = transport.base().to_string();
        let fetched = transport
            .get(DESCRIPTOR_PATH)
            .map_err(|e| translate(name, &url, e))?;
        let initialized = fetched.is_some();
        let descriptor = match fetched {
            Some(fetched) => {
                let descriptor: Descriptor =
                    serde_json::from_slice(&fetched.bytes).map_err(|e| {
                        RegistryError::Malformed {
                            registry: name.to_string(),
                            doc: DESCRIPTOR_PATH.to_string(),
                            detail: e.to_string(),
                        }
                    })?;
                // The one hard gate (§8.2). Refused rather than read optimistically: a
                // client that half-understands a newer format is worse than one that
                // declines, because it fails somewhere the user cannot connect to a version.
                if descriptor.schema_version > SCHEMA_VERSION {
                    return Err(RegistryError::SchemaTooNew {
                        registry: name.to_string(),
                        found: descriptor.schema_version,
                        supported: SCHEMA_VERSION,
                    });
                }
                descriptor
            }
            None => uninitialized(name, transport.writable()),
        };
        Ok(StaticHttp {
            name: name.to_string(),
            transport,
            descriptor,
            initialized,
        })
    }

    /// Whether this location already carries a registry descriptor. `false` means nothing
    /// has ever been published here — which a writable registry fixes on its first push,
    /// and a read-only one simply is not.
    pub fn initialized(&self) -> bool {
        self.initialized
    }

    fn index_path(name: &str) -> String {
        format!("v1/index/{name}.json")
    }

    fn artifact_path(name: &str, version: Version) -> String {
        format!("v1/artifacts/{name}/{name}-{version}.tgz")
    }

    fn changelog_path(name: &str, version: Version) -> String {
        format!("v1/changelogs/{name}/{version}.md")
    }

    fn url_for(&self, path: &str) -> String {
        format!("{}/{}", self.transport.base(), path)
    }

    fn name_of(&self, name: &str) -> RegistryResult<()> {
        checked_name(name)
            .map(|_| ())
            .map_err(|detail| RegistryError::Malformed {
                registry: self.name.clone(),
                doc: "package name".to_string(),
                detail,
            })
    }

    /// Read a package's index document, with the entity tag needed to swap it back.
    fn read_index(&self, name: &str) -> RegistryResult<(Option<PackageIndex>, Option<String>)> {
        let path = StaticHttp::index_path(name);
        let Some(fetched) = self.transport.get(&path).map_err(|e| self.io(e))? else {
            return Ok((None, None));
        };
        let index: PackageIndex =
            serde_json::from_slice(&fetched.bytes).map_err(|e| RegistryError::Malformed {
                registry: self.name.clone(),
                doc: path,
                detail: e.to_string(),
            })?;
        Ok((Some(index.sorted()), fetched.etag))
    }

    fn require_index(&self, name: &str) -> RegistryResult<(PackageIndex, Option<String>)> {
        match self.read_index(name)? {
            (Some(index), etag) => Ok((index, etag)),
            (None, _) => Err(RegistryError::NotFound {
                registry: self.name.clone(),
                what: name.to_string(),
            }),
        }
    }

    fn io(&self, e: TransportError) -> RegistryError {
        translate(&self.name, self.transport.base(), e)
    }

    /// Read, modify, compare-and-swap a package's index document, retrying the swap when
    /// someone else got there first.
    ///
    /// Everything that writes an index goes through here, so the push race is handled in
    /// exactly one place (§3.4) instead of being re-derived per caller. `edit` runs again
    /// on each attempt against the *freshly read* document, which is what makes the retry a
    /// merge rather than a clobber.
    fn update_index<T>(
        &self,
        name: &str,
        mut edit: impl FnMut(&mut PackageIndex) -> RegistryResult<T>,
    ) -> RegistryResult<T> {
        let path = StaticHttp::index_path(name);
        for _ in 0..CAS_ATTEMPTS {
            let (index, etag) = self.read_index(name)?;
            let mut index = index.unwrap_or_else(|| PackageIndex::new(name));
            let outcome = edit(&mut index)?;
            let bytes = to_json(&index);
            match self.transport.put_cas(&path, &bytes, etag.as_deref()) {
                Ok(()) => return Ok(outcome),
                Err(TransportError::Conflict) => continue,
                Err(e) => return Err(self.io(e)),
            }
        }
        Err(RegistryError::Conflict {
            registry: self.name.clone(),
            what: format!("the index for {name}"),
        })
    }

    /// Add a name to the enumeration document, if it is not already there.
    ///
    /// Best-effort by design: `packages.json` is a **convenience index**, and a publish that
    /// succeeded in every way that matters must not be reported as failed because this
    /// document was busy. A name missing from it costs enumeration, not resolution — an
    /// exact-name lookup reads `v1/index/<name>.json` directly.
    fn announce(&self, name: &str) -> Option<String> {
        for _ in 0..CAS_ATTEMPTS {
            let fetched = self.transport.get(PACKAGES_PATH).ok()?;
            let etag = fetched.as_ref().and_then(|f| f.etag.clone());
            let mut names: Vec<String> = match &fetched {
                Some(fetched) => serde_json::from_slice(&fetched.bytes).ok()?,
                None => Vec::new(),
            };
            if names.iter().any(|n| n == name) {
                return None;
            }
            names.push(name.to_string());
            names.sort();
            match self
                .transport
                .put_cas(PACKAGES_PATH, &to_json(&names), etag.as_deref())
            {
                Ok(()) => return None,
                Err(TransportError::Conflict) => continue,
                Err(e) => {
                    return Some(format!(
                        "{name} is published, but the registry's package list could not be \
                         updated ({e}); it will be listed on the next successful push"
                    ));
                }
            }
        }
        Some(format!(
            "{name} is published, but the registry's package list stayed busy; \
             it will be listed on the next successful push"
        ))
    }

    /// Write the descriptor if this registry has none yet, reporting whether it did.
    /// Called only from `publish` — reading a registry never creates anything.
    fn ensure_descriptor(&self) -> RegistryResult<bool> {
        let existing = self
            .transport
            .get(DESCRIPTOR_PATH)
            .map_err(|e| self.io(e))?;
        if existing.is_some() {
            return Ok(false);
        }
        let descriptor = Descriptor {
            schema_version: SCHEMA_VERSION,
            name: Some(self.name.clone()),
            capabilities: static_capabilities(),
        };
        match self
            .transport
            .put_new(DESCRIPTOR_PATH, &to_json(&descriptor))
        {
            Ok(()) => Ok(true),
            // Someone else initialized it between the check and the write. That is the
            // outcome we wanted, so it is not a failure.
            Err(TransportError::Exists) => Ok(false),
            Err(e) => Err(self.io(e)),
        }
    }
}

impl Registry for StaticHttp {
    fn name(&self) -> &str {
        &self.name
    }

    fn url(&self) -> &str {
        self.transport.base()
    }

    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn versions(&self, name: &str) -> RegistryResult<Vec<ReleaseMeta>> {
        self.name_of(name)?;
        let (index, _) = self.require_index(name)?;
        Ok(index.releases)
    }

    fn fetch(&self, name: &str, version: Version, into: &Path) -> RegistryResult<VerifiedArtifact> {
        self.name_of(name)?;
        let (index, _) = self.require_index(name)?;

        // Access is checked before the download, so a restricted package costs a document
        // read rather than a transfer — and so the hint reaches the user promptly (§8.5).
        // On a static host this is advisory and says so at `push` time; enforcing it here
        // anyway is what makes the flag mean the same thing on both kinds of registry.
        if !index.access.pullable {
            return Err(RegistryError::PullRestricted {
                registry: self.name.clone(),
                name: name.to_string(),
                hint: index.access.hint.clone(),
            });
        }
        let release = index.get(version).ok_or_else(|| RegistryError::NotFound {
            registry: self.name.clone(),
            what: format!("{name} {version}"),
        })?;

        let path = StaticHttp::artifact_path(name, version);
        let fetched = self
            .transport
            .get(&path)
            .map_err(|e| self.io(e))?
            .ok_or_else(|| RegistryError::Malformed {
                registry: self.name.clone(),
                doc: StaticHttp::index_path(name),
                detail: format!("it lists {version}, but {path} is not there"),
            })?;

        let actual = digest(&fetched.bytes);
        if actual != release.sha256 {
            // Nothing is written. The mismatch may be corruption or substitution, and the
            // two are indistinguishable from here — which is exactly why neither gets to
            // leave a file behind.
            return Err(RegistryError::ChecksumMismatch {
                name: name.to_string(),
                version,
                expected: release.sha256.clone(),
                actual,
            });
        }

        if let Some(parent) = into.parent() {
            std::fs::create_dir_all(parent).map_err(|e| RegistryError::Io {
                registry: self.name.clone(),
                detail: e.to_string(),
            })?;
        }
        std::fs::write(into, &fetched.bytes).map_err(|e| RegistryError::Io {
            registry: self.name.clone(),
            detail: e.to_string(),
        })?;

        Ok(VerifiedArtifact {
            name: name.to_string(),
            version,
            path: into.to_path_buf(),
            size: fetched.bytes.len() as u64,
            sha256: actual,
        })
    }

    fn list(&self) -> RegistryResult<Vec<PackageSummary>> {
        if !self.descriptor.capabilities.enumerable {
            return Err(RegistryError::Unsupported {
                registry: Some(self.name.clone()),
                op: "enumeration",
            });
        }
        let Some(fetched) = self.transport.get(PACKAGES_PATH).map_err(|e| self.io(e))? else {
            // Declared enumerable with nothing to enumerate: an empty registry, not a
            // broken one.
            return Ok(Vec::new());
        };
        let names: Vec<String> =
            serde_json::from_slice(&fetched.bytes).map_err(|e| RegistryError::Malformed {
                registry: self.name.clone(),
                doc: PACKAGES_PATH.to_string(),
                detail: e.to_string(),
            })?;

        let mut out = Vec::new();
        for name in names {
            // A name in the enumeration whose index document has gone is skipped rather
            // than fatal: the list is a convenience document that can lag reality, and one
            // stale entry must not make `registry list` unusable.
            let Ok((index, _)) = self.require_index(&name) else {
                continue;
            };
            if !index.access.listed {
                continue; // unlisted: fetchable by exact name, invisible here (§8.5)
            }
            let latest = index.latest();
            out.push(PackageSummary {
                latest,
                releases: index.releases.len(),
                // The newest release's description, not the oldest: a package's own
                // account of itself is whatever it most recently published.
                description: index.releases.last().and_then(|r| r.description.clone()),
                access: index.access,
                name,
            });
        }
        Ok(out)
    }

    fn publish(&self, request: PublishRequest<'_>) -> RegistryResult<Published> {
        let (name, version, artifact) = (request.name, request.version, request.artifact);
        self.name_of(name)?;
        if !self.transport.writable() {
            return Err(RegistryError::Unsupported {
                registry: Some(self.name.clone()),
                op: "publishing",
            });
        }
        self.ensure_descriptor()?;

        let bytes = std::fs::read(artifact).map_err(|e| RegistryError::Io {
            registry: self.name.clone(),
            detail: format!("{}: {e}", artifact.display()),
        })?;
        let sha256 = digest(&bytes);
        let size = bytes.len() as u64;

        // 1. The artifact, create-only. Storage adjudicates, so "already published" is a
        //    fact rather than a race.
        let artifact_path = StaticHttp::artifact_path(name, version);
        match self.transport.put_new(&artifact_path, &bytes) {
            Ok(()) => {}
            Err(TransportError::Exists) => {
                return Err(RegistryError::VersionExists {
                    registry: self.name.clone(),
                    name: name.to_string(),
                    version,
                });
            }
            Err(e) => return Err(self.io(e)),
        }

        // 2. The changelog. A collision is tolerated: a push retried after a failure
        //    between steps must be able to finish, and this document is per-version and
        //    deterministic, so what is already there is what we would write.
        if let Some(changelog) = request.changelog {
            let path = StaticHttp::changelog_path(name, version);
            match self.transport.put_new(&path, changelog.as_bytes()) {
                Ok(()) | Err(TransportError::Exists) => {}
                Err(e) => return Err(self.io(e)),
            }
        }

        // 3. The index document — the step that makes the release visible.
        let meta = ReleaseMeta {
            version,
            sha256: sha256.clone(),
            size,
            published_at: crate::clock::timestamp_utc(crate::clock::now()),
            yanked: false,
            deps: request.deps,
            description: request.description.map(str::to_string),
            changelog_excerpt: request.changelog_excerpt.map(str::to_string),
            signatures: None,
        };
        self.update_index(name, |index| {
            // Access is sticky: set at push, kept until changed (§8.5).
            if let Some(access) = &request.access {
                index.access = access.clone();
            }
            // The artifact write normally makes a refusal here impossible. It becomes
            // possible when the index lists a version whose artifact was deleted out of
            // band: `put_new` then succeeds with new bytes, this refuses, and the document
            // keeps the *old* digest — so every later fetch of that version fails its
            // checksum, permanently, after a publish that claimed to work. Refuse instead.
            match index.insert(meta.clone()) {
                true => Ok(()),
                false => Err(RegistryError::VersionExists {
                    registry: self.name.clone(),
                    name: name.to_string(),
                    version,
                }),
            }
        })?;

        // Enumeration is a convenience; a failure to update it is carried back as a warning
        // rather than failing a publish that has already landed.
        let warnings = self.announce(name).into_iter().collect();

        Ok(Published {
            name: name.to_string(),
            version,
            sha256,
            size,
            artifact_url: self.url_for(&artifact_path),
            warnings,
        })
    }

    fn yank(&self, name: &str, version: Version, yanked: bool) -> RegistryResult<()> {
        self.name_of(name)?;
        if !self.transport.writable() {
            return Err(RegistryError::Unsupported {
                registry: Some(self.name.clone()),
                op: "yanking",
            });
        }
        // Read once outside the swap so an unpublished version is reported as not found
        // rather than as a document this client would have to invent.
        self.require_index(name)?;
        self.update_index(name, |index| {
            match index.releases.iter_mut().find(|r| r.version == version) {
                // The artifact is untouched (§8.4): a lockfile pinning this version keeps
                // resolving, which is the entire difference between a yank and a deletion.
                Some(release) => {
                    release.yanked = yanked;
                    Ok(())
                }
                None => Err(RegistryError::NotFound {
                    registry: self.name.clone(),
                    what: format!("{name} {version}"),
                }),
            }
        })
    }
}

/// The capabilities a registry made of files actually has.
///
/// Written into every descriptor this client creates, and assumed for a registry that has
/// none yet. Note what is *not* claimed: no search (§10's ladder degrades to enumeration),
/// no server-side bump validation, and access enforcement declared **advisory** — anything
/// the host serves, its readers can fetch, and saying otherwise would turn a courtesy flag
/// into a security claim it cannot back (§8.5).
pub fn static_capabilities() -> Capabilities {
    Capabilities {
        search: SearchCapability::None,
        enumerable: true,
        publish: Some(PublishCapability::Put),
        yank: true,
        validate_bump: false,
        access_enforcement: AccessEnforcement::Advisory,
    }
}

/// The descriptor assumed for a registry that has not been initialized yet.
///
/// A writable one is treated as a registry-to-be and gets the full static capability set; a
/// read-only location with no descriptor is not a registry at all, so it is described as
/// able to do nothing — which turns "this https URL is not a registry" into an
/// `Unsupported`/`NotFound` at the operation, rather than a confusing failure later.
fn uninitialized(name: &str, writable: bool) -> Descriptor {
    Descriptor {
        schema_version: SCHEMA_VERSION,
        name: Some(name.to_string()),
        capabilities: match writable {
            true => static_capabilities(),
            false => Capabilities::default(),
        },
    }
}

/// Pretty-printed with a trailing newline: these documents are read by humans in bucket
/// browsers and diffed in object-version histories, and one release per line is what makes
/// that legible.
fn to_json<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap_or_else(|_| b"{}".to_vec());
    bytes.push(b'\n');
    bytes
}

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn translate(registry: &str, url: &str, e: TransportError) -> RegistryError {
    match e {
        TransportError::NotFound => RegistryError::NotFound {
            registry: registry.to_string(),
            what: url.to_string(),
        },
        TransportError::Unsupported(op) => RegistryError::Unsupported {
            registry: Some(registry.to_string()),
            op,
        },
        TransportError::Timeout(seconds) => RegistryError::Timeout {
            registry: registry.to_string(),
            seconds,
        },
        TransportError::Unreachable(detail) => RegistryError::Unreachable {
            registry: registry.to_string(),
            url: url.to_string(),
            detail,
        },
        TransportError::Exists => RegistryError::Io {
            registry: registry.to_string(),
            detail: "a document that must not exist already does".to_string(),
        },
        TransportError::Conflict => RegistryError::Conflict {
            registry: registry.to_string(),
            what: "a document".to_string(),
        },
        TransportError::Io(detail) => RegistryError::Io {
            registry: registry.to_string(),
            detail,
        },
    }
}
