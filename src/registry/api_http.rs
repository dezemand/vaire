//! [`Api`] — the `publish: api` registry (registry-server.md §2.2, §4).
//!
//! Reads are unauthenticated and identical to a static host's: a bucket a server fronts
//! still serves the five wire documents byte-exact (§2.1), so `Api` delegates every read
//! method to an internal [`StaticHttp`] rather than reimplementing them. Only `publish`
//! and `yank` differ — routed through the begin/commit protocol instead of conditional
//! `PUT`s, and carrying a bearer token on every request, because a write in this tier is
//! never anonymous (§2.2).
//!
//! Which implementation a configured registry gets is decided once, by what its
//! descriptor declares (`publish: put` vs `publish: api`) — never guessed, and never
//! something the caller chooses instead of the registry.

use std::path::Path;

use crate::model::Version;
use crate::userconfig;

use super::api_wire::{
    ApiErrorBody, ApiErrorKind, BeginPublishRequest, BeginPublishResponse, CommitPublishRequest,
    CommitPublishResponse, YankRequest,
};
use super::{
    Descriptor, PackageSummary, PublishRequest, Published, Registry, RegistryError, RegistryResult,
    SearchHit, StaticHttp, VerifiedArtifact, digest_hex,
};

/// How long to wait for the api-tier endpoints before calling the registry unreachable.
/// Separate from [`super::transport`]'s constant: begin/commit are small JSON exchanges,
/// not a document fetch, and a deployment behind a slower auth check should not have to
/// share a budget with plain reads.
const API_TIMEOUT_SECS: u64 = 30;

pub struct Api {
    name: String,
    reads: StaticHttp,
    agent: ureq::Agent,
}

impl Api {
    /// Open an api-tier registry at `url`. The descriptor is read once, exactly as
    /// [`StaticHttp::open`] does it — `Api` does not re-fetch it separately, so the two
    /// implementations never disagree about what one registry declared.
    pub fn open(name: &str, url: &str) -> RegistryResult<Api> {
        let reads = StaticHttp::open(name, url)?;
        Ok(Api {
            name: name.to_string(),
            reads,
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(API_TIMEOUT_SECS))
                .build(),
        })
    }

    fn base(&self) -> &str {
        self.reads.url()
    }

    fn url_for(&self, path: &str) -> String {
        format!("{}/{}", self.base(), path.trim_start_matches('/'))
    }

    /// The bearer token for this registry, from `VAIRE_TOKEN`/`credentials.toml`
    /// (userconfig's precedence rule, §2.3). `None` for a request the caller has not
    /// logged in for yet — sent anyway, so the server's 401 is what actually reports the
    /// problem, with whatever challenge it carries.
    fn token(&self) -> Option<String> {
        userconfig::registry_token(&self.name)
    }

    /// `POST` JSON with the registry's bearer token, returning the response body as text.
    /// `name`/`version` are only for [`Self::translate`]'s benefit, on the error path.
    ///
    /// `ureq`'s `json` cargo feature is not enabled in this crate — every other wire
    /// document already goes through `serde_json` directly, so a second JSON path would
    /// buy nothing — so requests and responses here are plain strings this method
    /// serializes and reads itself rather than `send_json`/`into_json`.
    fn post_json(
        &self,
        path: &str,
        name: &str,
        version: Version,
        body: &impl serde::Serialize,
    ) -> RegistryResult<String> {
        let text = serde_json::to_string(body).unwrap_or_default();
        let mut request = self.agent.post(&self.url_for(path));
        if let Some(token) = self.token() {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        let response = request
            .set("Content-Type", "application/json")
            .send_string(&text)
            .map_err(|e| self.translate(e, name, version))?;
        response.into_string().map_err(|e| RegistryError::Io {
            registry: self.name.clone(),
            detail: e.to_string(),
        })
    }

    /// Turn a failed api-tier call into a [`RegistryError`]. `name`/`version` fill in a
    /// [`RegistryError::VersionExists`], which the body alone does not carry.
    fn translate(&self, e: ureq::Error, name: &str, version: Version) -> RegistryError {
        match e {
            ureq::Error::Status(401, response) => {
                let login_command = response
                    .header("www-authenticate")
                    .and_then(parse_bearer_realm)
                    .unwrap_or_else(|| self.name.clone());
                RegistryError::LoginRequired {
                    registry: self.name.clone(),
                    login_command: format!("vaire registry login {login_command}"),
                }
            }
            ureq::Error::Status(403, response) => RegistryError::PermissionDenied {
                registry: self.name.clone(),
                action: error_detail(response, "this action"),
            },
            ureq::Error::Status(409, response) => match error_kind(response) {
                ApiErrorKind::VersionExists => RegistryError::VersionExists {
                    registry: self.name.clone(),
                    name: name.to_string(),
                    version,
                },
                _ => RegistryError::Conflict {
                    registry: self.name.clone(),
                    what: "a publish".to_string(),
                },
            },
            ureq::Error::Status(404, _) => RegistryError::NotFound {
                registry: self.name.clone(),
                what: "publish token".to_string(),
            },
            ureq::Error::Status(422, response) => RegistryError::Malformed {
                registry: self.name.clone(),
                doc: "staged artifact".to_string(),
                detail: error_detail(response, "checksum mismatch"),
            },
            ureq::Error::Status(code, response) => RegistryError::Unreachable {
                registry: self.name.clone(),
                url: self.base().to_string(),
                detail: error_detail(response, &format!("answered {code}")),
            },
            ureq::Error::Transport(t) => RegistryError::Unreachable {
                registry: self.name.clone(),
                url: self.base().to_string(),
                detail: t.to_string(),
            },
        }
    }
}

impl Registry for Api {
    fn name(&self) -> &str {
        &self.name
    }

    fn url(&self) -> &str {
        self.reads.url()
    }

    fn descriptor(&self) -> &Descriptor {
        self.reads.descriptor()
    }

    fn versions(&self, name: &str) -> RegistryResult<Vec<super::ReleaseMeta>> {
        self.reads.versions(name)
    }

    fn fetch(&self, name: &str, version: Version, into: &Path) -> RegistryResult<VerifiedArtifact> {
        self.reads.fetch(name, version, into)
    }

    fn list(&self) -> RegistryResult<Vec<PackageSummary>> {
        self.reads.list()
    }

    fn search(&self, query: &str, limit: usize) -> RegistryResult<Vec<SearchHit>> {
        self.reads.search(query, limit)
    }

    fn publish(&self, request: PublishRequest<'_>) -> RegistryResult<Published> {
        let (name, version) = (request.name, request.version);
        let bytes = std::fs::read(request.artifact).map_err(|e| RegistryError::Io {
            registry: self.name.clone(),
            detail: format!("{}: {e}", request.artifact.display()),
        })?;
        let sha256 = digest_hex(&bytes);

        // 1. begin — every fact about the release, asserted once.
        let begin_req = BeginPublishRequest {
            name: name.to_string(),
            version,
            sha256,
            size: bytes.len() as u64,
            deps: request.deps,
            description: request.description.map(str::to_string),
            changelog_excerpt: request.changelog_excerpt.map(str::to_string),
            changelog: request.changelog.map(str::to_string),
            access: request.access,
            claimed_bump: request.claimed_bump,
            prior_version: request.prior_version,
        };
        let response = self.post_json("v1/api/publish/begin", name, version, &begin_req)?;
        let begin: BeginPublishResponse =
            serde_json::from_str(&response).map_err(|e| RegistryError::Malformed {
                registry: self.name.clone(),
                doc: "publish/begin response".to_string(),
                detail: e.to_string(),
            })?;

        // 2. the upload itself — not part of the wire contract proper (§2.2.2); whatever
        //    the storage backend's own ack looks like is fine, only failure matters here.
        //    The bearer token still goes on it: an object store's presigned URL carries
        //    its own authorization and would ignore this header, but a deployment that
        //    routes the upload through its own endpoint (as a directory-backed registry
        //    does) may well check it, and there is no way to know which from here.
        let mut upload = self.agent.request(&begin.upload.method, &begin.upload.url);
        if let Some(token) = self.token() {
            upload = upload.set("Authorization", &format!("Bearer {token}"));
        }
        for (header, value) in &begin.upload.headers {
            upload = upload.set(header, value);
        }
        upload.send_bytes(&bytes).map_err(|e| RegistryError::Io {
            registry: self.name.clone(),
            detail: format!("uploading the artifact: {e}"),
        })?;

        // 3. commit — nothing left for the client to assert; the token names it all.
        let response = self.post_json(
            "v1/api/publish/commit",
            name,
            version,
            &CommitPublishRequest {
                publish_token: begin.publish_token,
            },
        )?;
        let committed: CommitPublishResponse =
            serde_json::from_str(&response).map_err(|e| RegistryError::Malformed {
                registry: self.name.clone(),
                doc: "publish/commit response".to_string(),
                detail: e.to_string(),
            })?;

        Ok(Published {
            name: committed.name,
            version: committed.version,
            sha256: committed.sha256,
            size: committed.size,
            artifact_url: committed.artifact_url,
            warnings: committed.warnings,
        })
    }

    fn yank(&self, name: &str, version: Version, yanked: bool) -> RegistryResult<()> {
        self.post_json(
            "v1/api/publish/yank",
            name,
            version,
            &YankRequest {
                name: name.to_string(),
                version,
                yanked,
            },
        )
        .map(|_| ())
    }
}

/// The realm out of a `WWW-Authenticate: Bearer realm="<registry>", …` challenge
/// (registry-server.md §2.3), so a `LoginRequired` error names the exact registry the
/// login command should target rather than whatever this client happened to call it.
fn parse_bearer_realm(header: &str) -> Option<String> {
    // The scheme token is stripped once, up front — not per comma-separated parameter,
    // which would leave it glued to the first one (`Bearer realm="central"`, not
    // `realm="central"`) and never match.
    let params = header.strip_prefix("Bearer").unwrap_or(header);
    params
        .split(',')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("realm="))
        .map(|realm| realm.trim_matches('"').to_string())
}

fn error_kind(response: ureq::Response) -> ApiErrorKind {
    response
        .into_string()
        .ok()
        .and_then(|text| serde_json::from_str::<ApiErrorBody>(&text).ok())
        .map(|body| body.error)
        .unwrap_or(ApiErrorKind::Io)
}

fn error_detail(response: ureq::Response, fallback: &str) -> String {
    let status = response.status();
    let text = response.into_string().unwrap_or_default();
    match serde_json::from_str::<ApiErrorBody>(&text) {
        Ok(body) => body
            .message
            .unwrap_or_else(|| format!("{status} {fallback}")),
        Err(_) => format!("{status} {fallback}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bearer_realm_is_extracted_from_a_challenge() {
        assert_eq!(
            parse_bearer_realm(r#"Bearer realm="central", scope="publish""#),
            Some("central".to_string())
        );
        assert_eq!(parse_bearer_realm("Bearer"), None);
    }
}
