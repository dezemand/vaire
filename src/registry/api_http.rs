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
            // Redirects are not followed on this agent (registry-server.md §4): every
            // call it makes carries a bearer token, and a `Location` is whoever answered
            // asking us to send it somewhere else. A registry that has moved is
            // reconfigured at its new address — `registry add` again — never followed
            // into on the strength of one response.
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(API_TIMEOUT_SECS))
                .redirects(0)
                .build(),
        })
    }

    /// The error for a 3xx answer, which [`Self::open`]'s agent hands back rather than
    /// following. `Unreachable` because that is what it is from here: the address on file
    /// does not answer the protocol, and the fix is a configuration change.
    fn redirected(&self, response: ureq::Response) -> RegistryError {
        let location = response
            .header("location")
            .unwrap_or("somewhere it did not say")
            .to_string();
        RegistryError::Unreachable {
            registry: self.name.clone(),
            url: self.base().to_string(),
            detail: format!(
                "answered {} redirecting to {location}; credentials are not sent along \
                 a redirect — re-add the registry at its current address",
                response.status()
            ),
        }
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
    ///
    /// A token that *is* on file is checked against the descriptor's `auth` block before
    /// it goes anywhere (§4): one minted for a different issuer or audience is refused
    /// here, on this side of the wire, rather than handed to a registry it was not
    /// issued for. Only `publish` and `yank` call this — reads never carry a token in
    /// this tier — and both run against exactly one registry the caller named or
    /// `select` picked unambiguously, which is what keeps the bare `VAIRE_TOKEN` scoped
    /// to a single target rather than every configured registry.
    fn token(&self) -> RegistryResult<Option<String>> {
        let Some(token) = userconfig::registry_token(&self.name) else {
            return Ok(None);
        };
        if let Some(auth) = &self.descriptor().auth
            && let Err(mismatch) = super::token::matches(auth, &token)
        {
            return Err(RegistryError::TokenMismatch {
                registry: self.name.clone(),
                claim: mismatch.claim,
                expected: mismatch.expected,
                found: mismatch.found,
            });
        }
        Ok(Some(token))
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
        if let Some(token) = self.token()? {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        let response = request
            .set("Content-Type", "application/json")
            .send_string(&text)
            .map_err(|e| self.translate(e, name, version))?;
        if (300..400).contains(&response.status()) {
            return Err(self.redirected(response));
        }
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
        if let Some(token) = self.token()? {
            upload = upload.set("Authorization", &format!("Bearer {token}"));
        }
        for (header, value) in &begin.upload.headers {
            upload = upload.set(header, value);
        }
        let uploaded = upload.send_bytes(&bytes).map_err(|e| RegistryError::Io {
            registry: self.name.clone(),
            detail: format!("uploading the artifact: {e}"),
        })?;
        if (300..400).contains(&uploaded.status()) {
            return Err(self.redirected(uploaded));
        }

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

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A registry that serves one descriptor and answers every api-tier call with a fixed
    /// status line and headers, counting those calls — enough to prove what the client
    /// does *before* and *instead of* trusting an answer.
    struct MockRegistry {
        base: String,
        api_calls: Arc<AtomicUsize>,
        _thread: std::thread::JoinHandle<()>,
    }

    fn serve(descriptor: &str, api_status: &str, api_headers: &str) -> MockRegistry {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock registry");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let descriptor = descriptor.to_string();
        let (api_status, api_headers) = (api_status.to_string(), api_headers.to_string());
        let api_calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&api_calls);
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let (status, headers, body) = match path {
                    "/.well-known/vaire-registry.json" => {
                        ("200 OK", String::new(), descriptor.as_bytes())
                    }
                    _ => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        (api_status.as_str(), api_headers.clone(), &b""[..])
                    }
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(body);
            }
        });
        MockRegistry {
            base,
            api_calls,
            _thread: thread,
        }
    }

    fn publish_request(artifact: &Path) -> PublishRequest<'_> {
        PublishRequest {
            name: "acme-core",
            version: Version::new(1, 0, 0),
            artifact,
            changelog: None,
            changelog_excerpt: None,
            deps: Default::default(),
            description: None,
            access: None,
            claimed_bump: None,
            prior_version: None,
        }
    }

    /// The unsigned-JWT helper from `token::tests`, reproduced rather than shared: what
    /// matters is the payload, and the signature is never read.
    fn jwt(payload: &str) -> String {
        fn enc(bytes: &[u8]) -> String {
            const ALPHABET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let mut buffer = 0u32;
                for (i, b) in chunk.iter().enumerate() {
                    buffer |= u32::from(*b) << (16 - 8 * i);
                }
                for i in 0..(chunk.len() + 1) {
                    out.push(ALPHABET[((buffer >> (18 - 6 * i)) & 63) as usize] as char);
                }
            }
            out
        }
        format!("{}.{}.{}", enc(b"{}"), enc(payload.as_bytes()), enc(b"sig"))
    }

    const WITH_IDENTITY: &str = r#"{"schema_version":1,"name":"central",
        "capabilities":{"publish":"api"},
        "auth":{"issuer":"https://login.example/tenant","audience":"api://vaire"}}"#;

    #[test]
    fn a_token_minted_for_another_registry_never_leaves_the_machine() {
        let _guard = crate::userconfig::VAIRE_TOKEN_ENV.lock().unwrap();
        let mock = serve(WITH_IDENTITY, "200 OK", "");
        // SAFETY: guarded by VAIRE_TOKEN_ENV — every test touching these variables
        // holds it.
        unsafe {
            std::env::set_var(
                "VAIRE_TOKEN_SCOPED_TEST",
                jwt(r#"{"iss":"https://login.elsewhere/","aud":"api://vaire"}"#),
            )
        };
        let api = Api::open("scoped-test", &mock.base).expect("descriptor reads");
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join("a.tgz");
        std::fs::write(&artifact, b"bytes").unwrap();

        let err = api.publish(publish_request(&artifact)).unwrap_err();
        unsafe { std::env::remove_var("VAIRE_TOKEN_SCOPED_TEST") };

        assert!(
            matches!(
                &err,
                RegistryError::TokenMismatch { claim: "iss", found, .. }
                    if found == "https://login.elsewhere/"
            ),
            "{err}"
        );
        assert_eq!(
            mock.api_calls.load(Ordering::SeqCst),
            0,
            "the token was refused before any api-tier request was made"
        );
    }

    #[test]
    fn a_redirect_is_reported_rather_than_followed_with_the_token() {
        let _guard = crate::userconfig::VAIRE_TOKEN_ENV.lock().unwrap();
        let mock = serve(
            WITH_IDENTITY,
            "307 Temporary Redirect",
            "Location: http://elsewhere.invalid/v1/api/publish/begin\r\n",
        );
        // An opaque token: nothing to compare, so it is the redirect that refuses.
        unsafe { std::env::set_var("VAIRE_TOKEN_REDIRECT_TEST", "opaque-token") };
        let api = Api::open("redirect-test", &mock.base).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join("a.tgz");
        std::fs::write(&artifact, b"bytes").unwrap();

        let err = api.publish(publish_request(&artifact)).unwrap_err();
        unsafe { std::env::remove_var("VAIRE_TOKEN_REDIRECT_TEST") };

        assert!(
            matches!(&err, RegistryError::Unreachable { detail, .. }
                if detail.contains("elsewhere.invalid") && detail.contains("redirect")),
            "{err}"
        );
        assert_eq!(
            mock.api_calls.load(Ordering::SeqCst),
            1,
            "exactly the one call that was answered with the redirect — nothing after it"
        );
    }
}
