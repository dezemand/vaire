//! What the client checks about a bearer token **before sending it** (registry-server.md
//! §4): that it was minted for the registry it is about to be attached to.
//!
//! This is not validation — the registry does that, against keys the client never sees —
//! and nothing here trusts a claim. It is the opposite direction: a token the *user* holds
//! for registry A must not be handed to registry B just because both are configured, and
//! the `iss`/`aud` claims are readable without a key, so the client can refuse on its own
//! side of the wire. A token that is not a JWT at all (an opaque token from a portal, or the
//! fixed tokens a local `vaire serve` accepts) says nothing about itself and is sent as-is.

use super::wire::Auth;

/// Why a token is not going to be sent to a registry declaring `auth`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub claim: &'static str,
    pub expected: String,
    pub found: String,
}

/// Compare a token's own `iss`/`aud` against what the registry's descriptor declared.
/// `Ok(())` for an opaque (non-JWT) token, which carries no claims to compare.
pub fn matches(auth: &Auth, token: &str) -> Result<(), Mismatch> {
    let Some(claims) = jwt_claims(token) else {
        return Ok(());
    };
    if let Some(iss) = claims.get("iss").and_then(|v| v.as_str())
        && iss.trim_end_matches('/') != auth.issuer.trim_end_matches('/')
    {
        return Err(Mismatch {
            claim: "iss",
            expected: auth.issuer.clone(),
            found: iss.to_string(),
        });
    }
    if let Some(audience) = &auth.audience
        && let Some(aud) = claims.get("aud")
    {
        // `aud` is a string or an array of strings (RFC 7519 §4.1.3).
        let carried: Vec<&str> = match aud {
            serde_json::Value::String(s) => vec![s.as_str()],
            serde_json::Value::Array(items) => items.iter().filter_map(|v| v.as_str()).collect(),
            _ => vec![],
        };
        if !carried.contains(&audience.as_str()) {
            return Err(Mismatch {
                claim: "aud",
                expected: audience.clone(),
                found: carried.join(", "),
            });
        }
    }
    Ok(())
}

/// The payload of a JWT, unverified. `None` for anything that is not three base64url
/// segments with a JSON object in the middle — which is what "opaque token" means here.
fn jwt_claims(token: &str) -> Option<serde_json::Value> {
    let mut parts = token.split('.');
    let (_header, payload, _signature) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let bytes = base64url_decode(payload)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.is_object().then_some(value)
}

/// RFC 4648 §5 (URL-safe alphabet, padding optional). Small enough that a dependency for
/// it would be the larger thing to review.
fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => break,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(audience: Option<&str>) -> Auth {
        Auth {
            issuer: "https://login.example/tenant/v2.0".into(),
            audience: audience.map(str::to_string),
            client_id: None,
            scopes: vec![],
            grants: vec![],
        }
    }

    /// An unsigned JWT with the given payload — the signature is never checked here, so a
    /// fixed one is as good as a real one.
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
        format!(
            "{}.{}.{}",
            enc(br#"{"alg":"none"}"#),
            enc(payload.as_bytes()),
            enc(b"sig")
        )
    }

    #[test]
    fn a_token_from_the_declared_issuer_is_sent() {
        let token = jwt(r#"{"iss":"https://login.example/tenant/v2.0/","aud":"api://vaire"}"#);
        assert_eq!(matches(&auth(Some("api://vaire")), &token), Ok(()));
        // No audience declared: nothing to compare it against.
        assert_eq!(matches(&auth(None), &token), Ok(()));
    }

    #[test]
    fn a_token_minted_for_another_registry_is_not_sent() {
        let other = jwt(r#"{"iss":"https://login.other/","aud":"api://vaire"}"#);
        let err = matches(&auth(None), &other).unwrap_err();
        assert_eq!(err.claim, "iss");
        assert_eq!(err.found, "https://login.other/");

        let wrong_aud = jwt(r#"{"iss":"https://login.example/tenant/v2.0","aud":["x","y"]}"#);
        let err = matches(&auth(Some("api://vaire")), &wrong_aud).unwrap_err();
        assert_eq!(err.claim, "aud");
        assert_eq!(err.found, "x, y");
    }

    #[test]
    fn an_opaque_token_carries_no_claims_and_is_sent_as_is() {
        assert_eq!(matches(&auth(Some("api://vaire")), "secret-token"), Ok(()));
        assert_eq!(matches(&auth(None), "a.b"), Ok(()));
        // Three segments, but the middle is not JSON: still opaque, not an error.
        assert_eq!(matches(&auth(None), "a.b.c"), Ok(()));
    }

    #[test]
    fn base64url_decodes_with_and_without_padding() {
        assert_eq!(base64url_decode("aGk").unwrap(), b"hi");
        assert_eq!(base64url_decode("aGk=").unwrap(), b"hi");
        assert_eq!(base64url_decode("_-8").unwrap(), [0xff, 0xef]);
        assert!(
            base64url_decode("a+b").is_none(),
            "standard alphabet is not url-safe"
        );
    }
}
