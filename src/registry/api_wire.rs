//! Wire documents for the `publish: api` tier — begin/commit publishing
//! (registry-server.md §2.2).
//!
//! [`wire`](super::wire) defines the five documents any static host can serve.
//! This module defines the three requests and their responses a server that also
//! *accepts* pushes exposes: `begin`, `commit`, `yank`. They are JSON, sent and read by
//! both the client (`Api`, this crate) and the server (the `vaire-registry` crate), and
//! kept in one place so the two cannot drift.
//!
//! Every field [`BeginPublishRequest`] carries is exactly what [`super::PublishRequest`]
//! already carries client-side — this module is that struct made a wire document, not a
//! second description of a publish.

use std::collections::BTreeMap;

use crate::model::{Bump, Version};

use super::Access;

/// `POST /v1/api/publish/begin` — everything the index entry will need, asserted once.
/// `commit` (below) takes nothing but a `publish_token`, precisely so a client cannot
/// smuggle a changed fact into the second request.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BeginPublishRequest {
    pub name: String,
    pub version: Version,
    pub sha256: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deps: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog_excerpt: Option<String>,
    /// The release record rendered as Markdown (registry.md §8.1). `None` only for a
    /// registry that never asked for one; the ordinary case always sends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_bump: Option<Bump>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_version: Option<Version>,
}

/// Where and how to upload the artifact — a single-use location outside the wire layout
/// (registry-server.md §2.2.2). `headers` are sent verbatim on the upload `PUT`; a
/// directory-backed test server has none, an object store typically demands
/// `content-type` and its own signature headers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UploadInstructions {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// RFC 3339 UTC. A client past this reruns `begin`, which is safe — the server's own
    /// "already published" check has not changed.
    pub expires_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BeginPublishResponse {
    pub upload: UploadInstructions,
    /// Opaque. References the exact `begin` call — the declared coordinates, the
    /// staging location, an expiry — so `commit` needs nothing else to finish the job.
    pub publish_token: String,
}

/// `POST /v1/api/publish/commit`. Deliberately just the token: every fact about the
/// release was asserted at `begin`, and commit has nothing left for a client to edit.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommitPublishRequest {
    pub publish_token: String,
}

/// The shape of [`super::Published`], made a wire document so a client's success
/// handling is identical whether the transport was `put` or `api`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommitPublishResponse {
    pub name: String,
    pub version: Version,
    pub sha256: String,
    pub size: u64,
    pub artifact_url: String,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// `POST /v1/api/publish/yank`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct YankRequest {
    pub name: String,
    pub version: Version,
    pub yanked: bool,
}

/// The body every non-2xx response from the API tier carries, mapped onto the same
/// [`super::RegistryError`] vocabulary the static-host transport already uses
/// (registry-server.md §2.2.1's error table) — so a fan-out engine dispatches on one
/// set of dispositions regardless of which publish capability answered.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiErrorKind {
    Malformed,
    Unreachable,
    VersionExists,
    NotFound,
    ChecksumMismatch,
    Io,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_begin_request_round_trips_with_every_optional_field_present() {
        let req = BeginPublishRequest {
            name: "togaf".into(),
            version: Version::new(10, 0, 1),
            sha256: "0".repeat(64),
            size: 14_126_567,
            deps: BTreeMap::from([("acme-glossary".to_string(), "^1".to_string())]),
            description: Some("…".into()),
            changelog_excerpt: None,
            changelog: Some("## 10.0.1\n\n…".into()),
            access: Some(Access {
                listed: true,
                pullable: true,
                hint: None,
            }),
            claimed_bump: Some(Bump::Patch),
            prior_version: Some(Version::new(10, 0, 0)),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: BeginPublishRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "togaf");
        assert_eq!(back.claimed_bump, Some(Bump::Patch));
        assert_eq!(back.prior_version, Some(Version::new(10, 0, 0)));
    }

    #[test]
    fn a_begin_request_with_every_optional_field_absent_still_parses() {
        let json = r#"{"name":"a","version":"1.0.0","sha256":"ab","size":1}"#;
        let req: BeginPublishRequest = serde_json::from_str(json).unwrap();
        assert!(req.deps.is_empty());
        assert!(req.access.is_none());
        assert!(req.claimed_bump.is_none());
    }

    #[test]
    fn an_error_body_round_trips_every_kind() {
        for kind in [
            ApiErrorKind::Malformed,
            ApiErrorKind::Unreachable,
            ApiErrorKind::VersionExists,
            ApiErrorKind::NotFound,
            ApiErrorKind::ChecksumMismatch,
            ApiErrorKind::Io,
        ] {
            let body = ApiErrorBody {
                error: kind,
                message: Some("detail".into()),
            };
            let json = serde_json::to_string(&body).unwrap();
            let back: ApiErrorBody = serde_json::from_str(&json).unwrap();
            assert_eq!(back.error, kind);
        }
    }
}
