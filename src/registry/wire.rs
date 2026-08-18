//! The registry wire documents (registry.md §9–§9.1).
//!
//! Four JSON shapes, each one file a dumb host can serve. They are defined here as serde
//! types rather than assembled ad hoc, for the reason every wire format eventually learns:
//! the *reader* is the contract. A field this client does not know is ignored (so a newer
//! registry stays readable within a schema version), a field it does know is typed (so a
//! malformed version fails at the document boundary rather than three layers down), and a
//! field it writes is written the same way every time.
//!
//! ## Forward compatibility has exactly one hard gate
//!
//! [`Descriptor::schema_version`]. Everything else is soft: absent booleans default to the
//! permissive value, absent objects to their defaults, unknown keys are dropped. That is
//! deliberate asymmetry — an additive change must not break an old client, and a
//! *structural* change must not be half-understood by one. A `schema_version` bump is the
//! only way to say "stop reading".
//!
//! ## Ordering is not incidental
//!
//! `releases` is written oldest-first and re-sorted on read. An index document is
//! append-mostly and read by humans as often as by clients, and a stable order is what
//! makes its diffs legible — which matters when the document lives in a bucket whose only
//! audit trail is object versioning.

use std::collections::BTreeMap;

use crate::model::Version;

/// `/.well-known/vaire-registry.json` — what this registry is and what it can do (§8.2).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Descriptor {
    pub schema_version: u32,
    /// The registry's own name for itself. Advisory: locally, a registry is known by
    /// whatever `vaire registry add` called it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// What the registry claims it can do. Every field defaults to the **least** capable
/// value, so a descriptor that predates a capability is read as not having it rather than
/// as having it silently.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    pub search: SearchCapability,
    #[serde(default)]
    pub enumerable: bool,
    /// How to publish: `"put"` (conditional writes against object storage) or `"api"`.
    /// Absent means the registry is read-only to this client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<PublishCapability>,
    #[serde(default)]
    pub yank: bool,
    /// Whether the registry recomputes the release classifier server-side and can reject a
    /// bump that the diff does not support. Static hosts do not (§8.4).
    #[serde(default)]
    pub validate_bump: bool,
    #[serde(default)]
    pub access_enforcement: AccessEnforcement,
}

impl Default for Capabilities {
    fn default() -> Capabilities {
        Capabilities {
            search: SearchCapability::None,
            enumerable: false,
            publish: None,
            yank: false,
            validate_bump: false,
            access_enforcement: AccessEnforcement::Advisory,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchCapability {
    #[default]
    None,
    Lexical,
    Hybrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublishCapability {
    /// Conditional `PUT`s: create-only for artifacts, compare-and-swap for index
    /// documents. What a bucket or a filesystem gives us with no server code.
    Put,
    /// A mediating service with its own endpoints.
    Api,
}

/// Whether the access flags are policy signalling or an actual control (§8.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessEnforcement {
    /// The bucket serves what it serves; the flags are a courtesy. **The default**, because
    /// assuming enforcement that is not there is the failure mode with consequences.
    #[default]
    Advisory,
    Enforced,
}

/// `/v1/index/<name>.json` — a package's release index (§8.3).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PackageIndex {
    pub name: String,
    #[serde(default)]
    pub access: Access,
    #[serde(default)]
    pub releases: Vec<ReleaseMeta>,
}

impl PackageIndex {
    pub fn new(name: &str) -> PackageIndex {
        PackageIndex {
            name: name.to_string(),
            access: Access::default(),
            releases: Vec::new(),
        }
    }

    pub fn get(&self, version: Version) -> Option<&ReleaseMeta> {
        self.releases.iter().find(|r| r.version == version)
    }

    /// The highest version that is not yanked. `None` when every release is yanked, which
    /// is a different state from an absent package and is reported as such.
    pub fn latest(&self) -> Option<Version> {
        self.releases
            .iter()
            .filter(|r| !r.yanked)
            .map(|r| r.version)
            .max()
    }

    /// Insert a release, keeping the vector ordered by version.
    ///
    /// Returns `false` — changing nothing — when that version is already present.
    /// Immutability is enforced by the *write* (a create-only artifact `PUT`), so this is
    /// not the guard; it is the guard against a merge that would leave the document holding
    /// one version twice, which no reader is prepared for.
    pub fn insert(&mut self, release: ReleaseMeta) -> bool {
        if self.get(release.version).is_some() {
            return false;
        }
        let at = self
            .releases
            .partition_point(|r| r.version < release.version);
        self.releases.insert(at, release);
        true
    }

    /// Sort releases oldest-first. Applied on read as well as write: a document edited by
    /// hand, or by an older client, should not change how anything resolves.
    pub fn sorted(mut self) -> PackageIndex {
        self.releases.sort_by_key(|r| r.version);
        self
    }
}

/// One release, as the index document records it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReleaseMeta {
    pub version: Version,
    pub sha256: String,
    pub size: u64,
    /// RFC 3339 UTC.
    pub published_at: String,
    #[serde(default)]
    pub yanked: bool,
    /// The release's declared dependencies. **Here, not in the artifact**, so transitive
    /// resolution never downloads a package to read its manifest (decision 9).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deps: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// For a major, the invalidated-assumptions summary. Its job is letting a dependent
    /// decide about re-confirmation **before fetching anything** (§8.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog_excerpt: Option<String>,
    /// A **reserved slot only**. The field exists so that adding signing later is not a
    /// schema break; no scheme is designed, and nothing reads this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signatures: Option<serde_json::Value>,
}

/// Who may see and who may fetch a package, per registry (§8.5).
///
/// Two orthogonal axes, **both defaulting to `true` when absent**, so every index document
/// written before access existed stays valid and open. Three served states fall out: open
/// (both), restricted (listed, not pullable), unlisted (pullable by exact name, invisible
/// to search). Entirely-private is not a flag — it is absence, or a differently-ACL'd
/// registry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Access {
    #[serde(default = "yes")]
    pub listed: bool,
    #[serde(default = "yes")]
    pub pullable: bool,
    /// Where to ask, when it is not pullable. This is the *point* of restricted-listed
    /// rather than a courtesy on top of it: the state exists to prevent duplicated work by
    /// routing someone to the owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

fn yes() -> bool {
    true
}

/// Check that a package name is safe to place in a URL path, returning it unchanged.
///
/// The registry is where a declared name first becomes **a path on someone else's disk**.
/// `knowledge.toml` calls `name` "a slug" and nothing has ever enforced that, which is fine
/// while a name only keys a local map — and is not fine the moment it is joined onto a
/// registry root, where `../../` would write outside it and a name differing only in case
/// would collide on one filesystem and not another.
///
/// So the grammar is enforced here rather than at the manifest: lowercase ASCII
/// alphanumerics, `-`, `_`, `.`, starting with a letter or digit. It is the intersection of
/// what every plausible host treats as one ordinary path segment.
pub fn checked_name(name: &str) -> Result<&str, String> {
    const MAX: usize = 128;
    let bad = |why: &str| {
        Err(format!(
            "'{name}' is not a usable package name on a registry: {why}"
        ))
    };
    if name.is_empty() {
        return bad("it is empty");
    }
    if name.len() > MAX {
        return bad(&format!("it is longer than {MAX} characters"));
    }
    if !name
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return bad("it must start with a lowercase letter or a digit");
    }
    match name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.'))
    {
        true => Ok(name),
        false => bad("only lowercase letters, digits, '-', '_' and '.' are allowed"),
    }
}

impl Default for Access {
    fn default() -> Access {
        Access {
            listed: true,
            pullable: true,
            hint: None,
        }
    }
}

impl Access {
    /// The three named states, as `vaire push --access` spells them.
    pub fn state(&self) -> &'static str {
        match (self.listed, self.pullable) {
            (true, true) => "open",
            (true, false) => "restricted",
            (false, true) => "unlisted",
            // Not reachable through `--access`, but a hand-edited document can say it, and
            // the honest name for "in the index, invisible, unfetchable" is not one of the
            // three.
            (false, false) => "closed",
        }
    }

    pub fn parse(state: &str, hint: Option<String>) -> Option<Access> {
        let (listed, pullable) = match state {
            "open" => (true, true),
            "restricted" => (true, false),
            "unlisted" => (false, true),
            _ => return None,
        };
        Some(Access {
            listed,
            pullable,
            hint,
        })
    }

    pub const STATES: [&'static str; 3] = ["open", "restricted", "unlisted"];
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str) -> ReleaseMeta {
        ReleaseMeta {
            version: version.parse().unwrap(),
            sha256: "0".repeat(64),
            size: 1,
            published_at: "2026-08-01T09:30:00Z".into(),
            yanked: false,
            deps: BTreeMap::new(),
            description: None,
            changelog_excerpt: None,
            signatures: None,
        }
    }

    #[test]
    fn an_index_document_from_a_future_client_still_reads() {
        // Unknown keys are dropped, known ones are typed, absent optionals default.
        let json = r#"{
            "name": "acme-core",
            "releases": [
                {"version": "1.4.2", "sha256": "ab", "size": 10,
                 "published_at": "2026-08-01T09:30:00Z",
                 "attestations": {"in-toto": "…"}}
            ],
            "mirrors": ["https://elsewhere.example"]
        }"#;
        let index: PackageIndex = serde_json::from_str(json).unwrap();
        assert_eq!(index.releases[0].version, Version::new(1, 4, 2));
        // Absent access is open, which is what keeps pre-access documents valid.
        assert!(index.access.listed && index.access.pullable);
        assert!(!index.releases[0].yanked);
    }

    #[test]
    fn a_malformed_version_fails_at_the_document_boundary() {
        let json = r#"{"name":"acme-core","releases":[
            {"version":"1.4","sha256":"ab","size":1,"published_at":"x"}]}"#;
        assert!(serde_json::from_str::<PackageIndex>(json).is_err());
    }

    #[test]
    fn capabilities_default_to_the_least_capable_reading() {
        let descriptor: Descriptor = serde_json::from_str(r#"{"schema_version":1}"#).unwrap();
        assert_eq!(descriptor.capabilities.search, SearchCapability::None);
        assert!(!descriptor.capabilities.enumerable);
        assert!(descriptor.capabilities.publish.is_none());
        // The consequential one: never assume enforcement that is not there.
        assert_eq!(
            descriptor.capabilities.access_enforcement,
            AccessEnforcement::Advisory
        );
    }

    #[test]
    fn releases_stay_ordered_and_a_duplicate_is_refused() {
        let mut index = PackageIndex::new("acme-core");
        assert!(index.insert(release("1.10.0")));
        assert!(index.insert(release("1.9.0")));
        assert!(index.insert(release("2.0.0")));
        assert!(!index.insert(release("1.9.0")), "already published");
        let versions: Vec<String> = index
            .releases
            .iter()
            .map(|r| r.version.to_string())
            .collect();
        assert_eq!(versions, ["1.9.0", "1.10.0", "2.0.0"]);
    }

    #[test]
    fn latest_skips_yanks_and_distinguishes_all_yanked_from_absent() {
        let mut index = PackageIndex::new("acme-core");
        index.insert(release("1.0.0"));
        let mut newer = release("1.1.0");
        newer.yanked = true;
        index.insert(newer);
        assert_eq!(index.latest(), Some(Version::new(1, 0, 0)));

        let mut all_yanked = PackageIndex::new("acme-core");
        let mut only = release("1.0.0");
        only.yanked = true;
        all_yanked.insert(only);
        assert_eq!(all_yanked.latest(), None);
        assert_eq!(all_yanked.releases.len(), 1, "still a published package");
    }

    #[test]
    fn a_name_cannot_traverse_out_of_the_registry_root() {
        assert_eq!(checked_name("acme-core"), Ok("acme-core"));
        assert_eq!(checked_name("scania.it_2"), Ok("scania.it_2"));
        for bad in [
            "../../etc/passwd",
            "a/b",
            "..",
            ".hidden",
            "-leading-dash",
            "Acme-Core",
            "acme core",
            "",
        ] {
            assert!(checked_name(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn access_states_round_trip() {
        for state in Access::STATES {
            assert_eq!(Access::parse(state, None).unwrap().state(), state);
        }
        assert!(Access::parse("private", None).is_none());
        let restricted = Access::parse("restricted", Some("ask #team".into())).unwrap();
        assert!(restricted.listed && !restricted.pullable);
    }
}
