//! `vaire registry add|list|rm|show` (cli.md §4.9) — the remotes this machine publishes to
//! and pulls from (registry.md §9).
//!
//! The third of the three "add"s, and the grammar is what keeps them apart: `add` declares
//! a dependency, `catalog add` records a package on this machine, `registry add` configures
//! a remote. All three are noun-grouped except the one that is workflow.
//!
//! Unlike the catalog's package sightings, nothing here is ambient. A package can be
//! stumbled across — it is a directory that happens to have a manifest — but a registry is
//! a decision, so these rows are written only by these commands and are never observations
//! that heal themselves.
//!
//! Reachability is probed, never required. `registry add` reports what answered, and
//! records the row either way: configuring a remote you cannot currently reach (a VPN is
//! down, the bucket is not created yet) is an ordinary thing to do, and refusing it would
//! mean the tool insisting the network exist before it will remember a URL.

use std::io::Read;
use std::path::Path;

use crate::catalog::{Catalog, KIND_API, KIND_STATIC, RegistryRow};
use crate::error::{Result, VaireError};
use crate::output::{
    RegistryAddOutput, RegistryListOutput, RegistryLoginOutput, RegistryLogoutOutput,
    RegistryRemoveOutput, RegistryShowOutput, SignedIn,
};
use crate::registry::wire::PublishCapability;
use crate::registry::{Api, Registry, StaticHttp, transport};
use crate::userconfig;

/// `vaire registry add <name> <url> [--priority N] [--no-search]`.
pub fn add(
    home: &Path,
    name: &str,
    url: &str,
    priority: i64,
    search_by_default: bool,
) -> Result<RegistryAddOutput> {
    // Trimmed once, here, and stored trimmed: the name is the primary key, so recording
    // `"  lab  "` would leave `registry show lab`, `push --registry lab` and `registry rm
    // lab` all failing against a listing that reads exactly like `lab`.
    let name = name.trim();
    if name.is_empty() {
        return Err(VaireError::Usage("a registry needs a name".into()));
    }
    let url = transport::normalize_url(url)
        .map_err(|e| VaireError::Usage(format!("{url} is not a usable registry location: {e}")))?;
    let row = RegistryRow {
        name: name.to_string(),
        url: url.clone(),
        kind: KIND_STATIC.to_string(),
        priority,
        search_by_default,
    };
    // What was there before, so a refused re-add of an existing name can put it back
    // rather than deleting a row the user did not ask to lose.
    let previous = {
        let catalog = Catalog::open(home)?;
        let registries = catalog.registries()?;
        // `VAIRE_TOKEN_<NAME>` folds every non-alphanumeric to `_`, so `prod.eu` and
        // `prod-eu` would read the same variable — and one registry's token would be
        // sent to the other. Refused here, where the second name is chosen, rather
        // than resolved by whichever row the lookup happens to hit first.
        let env = userconfig::registry_token_env(name);
        if let Some(other) = registries
            .iter()
            .find(|r| r.name != name && userconfig::registry_token_env(&r.name) == env)
        {
            return Err(VaireError::Usage(format!(
                "'{name}' and the configured registry '{}' would both read their token \
                 from {env} — pick a name that differs in more than punctuation",
                other.name
            )));
        }
        let previous = registries.into_iter().find(|r| r.name == name);
        catalog.add_registry(&row)?;
        previous
    };

    // Probe after recording, so a failed probe cannot lose the configuration the user just
    // asked for. What comes back is reported as information, never as a verdict.
    let probe = match StaticHttp::open(name, &url) {
        Ok(registry) => {
            // The one thing a probe *can* veto (registry-server.md §2.3): a registry that
            // declares identity over plain `http://`. A bearer token is a bare credential,
            // so a location that would have this client send one in clear is not
            // recorded at all — the row is withdrawn again rather than left for a later
            // `push` to discover the problem with a token already on the wire.
            if let Some(why) = insecure_identity(&url, registry.descriptor()) {
                let catalog = Catalog::open(home)?;
                match &previous {
                    Some(row) => catalog.add_registry(row)?,
                    None => {
                        catalog.forget_registry(name)?;
                    }
                }
                return Err(VaireError::Usage(why));
            }
            let publish = registry.descriptor().capabilities.publish;
            let writable = publish.is_some();
            // The kind this client will actually construct on every later `open()` —
            // decided once, here, from what the registry declared, and corrected in the
            // catalog below. `registry add` is the one place this probe runs unforced,
            // so it is the one place a registry that has since grown `publish: api`
            // (or lost it) gets noticed; `registry show`/`push` trust the stored kind
            // rather than re-probing on every call.
            if publish == Some(PublishCapability::Api) {
                let catalog = Catalog::open(home)?;
                catalog.add_registry(&RegistryRow {
                    name: name.to_string(),
                    url: url.clone(),
                    kind: KIND_API.to_string(),
                    priority,
                    search_by_default,
                })?;
            }
            // A location with no descriptor has never been published to. Reported as such
            // rather than as "0 packages": an empty registry and a directory that is not a
            // registry yet look identical in a count and are not the same situation.
            //
            // The same care applies to a failed enumeration. `None` here already means
            // "cannot enumerate", so silently mapping a broken `packages.json` onto it
            // would describe a registry as unable to do the thing it declares it can.
            let (packages, enumeration) = match registry.initialized() {
                false => (None, None),
                true => match registry.list() {
                    Ok(packages) => (Some(packages.len()), None),
                    Err(e) => (None, Some(e.to_string())),
                },
            };
            Ok(Probe {
                writable,
                initialized: registry.initialized(),
                packages,
                enumeration,
            })
        }
        Err(e) => Err(e.to_string()),
    };
    Ok(RegistryAddOutput {
        name: name.to_string(),
        url,
        reachable: probe.is_ok(),
        packages: probe.as_ref().ok().and_then(|p| p.packages),
        initialized: probe.as_ref().is_ok_and(|p| p.initialized),
        writable: probe.as_ref().is_ok_and(|p| p.writable),
        enumeration: probe.as_ref().ok().and_then(|p| p.enumeration.clone()),
        note: probe.err(),
    })
}

/// Why `url` may not be recorded, when it takes a bearer token (an `auth` block, or
/// `publish: api`, whose writes are never anonymous) and the scheme would carry that
/// token in clear — `None` when it may. The loopback exception is
/// [`crate::registry::token::cleartext`]'s.
fn insecure_identity(url: &str, descriptor: &crate::registry::Descriptor) -> Option<String> {
    if !takes_identity(descriptor) || !crate::registry::token::cleartext(url) {
        return None;
    }
    let what = match &descriptor.auth {
        Some(auth) => format!("declares an identity provider ({})", auth.issuer),
        None => "publishes through the api tier, which never accepts an anonymous write".into(),
    };
    Some(format!(
        "{url} {what} but is served over plain http:// — a bearer token sent there would \
         travel in clear, so this registry is not recorded; use its https:// address"
    ))
}

struct Probe {
    writable: bool,
    initialized: bool,
    packages: Option<usize>,
    /// Why enumeration failed, when it did. Distinct from `packages: None`, which means
    /// the registry cannot enumerate at all.
    enumeration: Option<String>,
}

/// `vaire registry list`.
pub fn list(home: &Path) -> Result<RegistryListOutput> {
    let catalog = Catalog::open(home)?;
    Ok(RegistryListOutput {
        catalog: catalog.path().display().to_string(),
        registries: catalog.registries()?,
    })
}

/// `vaire registry rm <name>`.
pub fn remove(home: &Path, name: &str) -> Result<RegistryRemoveOutput> {
    let catalog = Catalog::open(home)?;
    Ok(RegistryRemoveOutput {
        name: name.to_string(),
        removed: catalog.forget_registry(name)?,
    })
}

/// `vaire registry show <name>` — what this registry is, and what it holds.
///
/// The one command that reads the wire without publishing anything, so it doubles as the
/// diagnostic: a descriptor that will not parse, a schema this vaire is too old for, or a
/// URL that answers nothing all surface here rather than at the moment someone tries to
/// push.
pub fn show(home: &Path, name: &str) -> Result<RegistryShowOutput> {
    let row = find(home, name)?;
    let registry = open(&row)?;
    let descriptor = registry.descriptor().clone();
    // Enumeration is capability-gated, so a registry that cannot list is reported as
    // "cannot", not as empty. The two look identical in a bare count and mean opposite
    // things to someone deciding whether to trust a search result.
    //
    // A registry that *declares* `enumerable` and then fails to enumerate is a third
    // thing, and the one this command exists to surface: the error propagates rather than
    // collapsing into "cannot", which would describe the fault as a capability.
    let packages = match descriptor.capabilities.enumerable {
        true => Some(registry.list()?),
        false => None,
    };
    // Login state is reported only where identity means something: a static host has
    // nothing to be signed in to, and "no" against it would read as something to fix.
    let signed_in = takes_identity(&descriptor).then(|| {
        let source = userconfig::registry_token_source(&row.name);
        SignedIn {
            signed_in: source.is_some(),
            source,
        }
    });
    Ok(RegistryShowOutput {
        name: row.name,
        url: row.url,
        priority: row.priority,
        search_by_default: row.search_by_default,
        schema_version: descriptor.schema_version,
        declared_name: descriptor.name,
        capabilities: descriptor.capabilities,
        auth: descriptor.auth,
        signed_in,
        packages,
    })
}

/// Whether a registry has any use for a token: it declares an identity provider, or it
/// publishes through the api tier, whose writes are never anonymous (registry-server.md
/// §2.2). The second without the first is a registry that validates tokens it does not
/// advertise an issuer for — a local `vaire serve` with fixed tokens, or a portal-issued
/// token — and `--token-stdin` is exactly the login it supports.
fn takes_identity(descriptor: &crate::registry::Descriptor) -> bool {
    descriptor.auth.is_some() || descriptor.capabilities.publish == Some(PublishCapability::Api)
}

/// The configured registry called `name`. Trimmed, as `add` trimmed it before storing:
/// the name is the key, and `" central "` must find `central`.
pub fn find(home: &Path, name: &str) -> Result<RegistryRow> {
    let name = name.trim();
    let catalog = Catalog::open(home)?;
    let registries = catalog.registries()?;
    registries
        .iter()
        .find(|r| r.name == name)
        .cloned()
        .ok_or_else(|| {
            VaireError::Usage(match registries.is_empty() {
                true => format!(
                    "no registry called '{name}' — none is configured yet \
                     (`vaire registry add <name> <url>`)"
                ),
                false => format!(
                    "no registry called '{name}'; configured: {}",
                    registries
                        .iter()
                        .map(|r| r.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            })
        })
}

/// The registry a command that needs exactly one should use.
///
/// Named explicitly, or inferred — and the inference refuses to guess. One configured
/// registry is unambiguous; several are only unambiguous when one has strictly the highest
/// priority, which is what that column is for. Anything else asks, because publishing to
/// the wrong registry is not an error anyone can take back: the artifact is immutable, and
/// the only remedy is a yank that stays visible.
pub fn select(home: &Path, requested: Option<&str>) -> Result<RegistryRow> {
    if let Some(name) = requested {
        return find(home, name);
    }
    let registries = {
        let catalog = Catalog::open(home)?;
        catalog.registries()?
    };
    match registries.as_slice() {
        [] => Err(VaireError::Usage(
            "no registry is configured — `vaire registry add <name> <url>` \
             (a directory works: `vaire registry add lab ./registry`)"
                .into(),
        )),
        [only] => Ok(only.clone()),
        // Ordered by priority already, so the two highest are the only ones that can tie.
        [first, second, ..] if first.priority > second.priority => Ok(first.clone()),
        many => Err(VaireError::Usage(format!(
            "several registries are configured and none has the highest priority — \
             name one with `--registry`: {}",
            many.iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Construct the client for a configured registry.
pub fn open(row: &RegistryRow) -> Result<Box<dyn Registry>> {
    match row.kind.as_str() {
        KIND_STATIC => Ok(Box::new(StaticHttp::open(&row.name, &row.url)?)),
        KIND_API => Ok(Box::new(Api::open(&row.name, &row.url)?)),
        other => Err(VaireError::Registry(format!(
            "registry '{}' is recorded as kind '{other}', which this vaire cannot speak — \
             re-add it, or run `vaire upgrade`",
            row.name
        ))),
    }
}

/// `vaire registry login <name> [--token-stdin]`.
///
/// The device-code flow of registry-server.md §2.3 needs a chosen identity provider to
/// run against and is not wired yet (§9) — `--token-stdin` is the honest present-tense
/// surface: paste a token however the registry's own portal issued one, and it is stored
/// exactly where the device flow will store its own tokens later, so nothing above this
/// (push, yank) needs to change when it lands.
///
/// The registry is opened first, for two refusals that belong here rather than at the
/// next `push`: a registry with no use for a token at all (registry-server.md §4 — "this
/// registry is anonymous"), and a pasted token whose own claims say it was minted for a
/// different party than the descriptor names.
pub fn login(home: &Path, name: &str, token_stdin: bool) -> Result<RegistryLoginOutput> {
    // The row's own (trimmed) name keys the token, so a login for `" central "` is
    // stored where `push --registry central` will look.
    let row = find(home, name)?;
    let name = row.name.as_str();
    let registry = open(&row)?;
    let descriptor = registry.descriptor();
    if !takes_identity(descriptor) {
        return Err(VaireError::Usage(format!(
            "registry '{name}' is anonymous — it declares no identity provider and does \
             not publish through the api tier, so there is nothing to sign in to"
        )));
    }
    if !token_stdin {
        return Err(VaireError::Usage(format!(
            "interactive sign-in is not available in this vaire yet — obtain a token from \
             '{name}' and run `vaire registry login {name} --token-stdin`"
        )));
    }
    let mut token = String::new();
    std::io::stdin()
        .read_to_string(&mut token)
        .map_err(|e| VaireError::Usage(format!("could not read the token from stdin: {e}")))?;
    let token = token.trim();
    if token.is_empty() {
        return Err(VaireError::Usage("no token was given on stdin".into()));
    }
    if let Some(auth) = &descriptor.auth
        && let Err(mismatch) = crate::registry::token::matches(auth, token)
    {
        return Err(VaireError::Usage(format!(
            "that token was issued with {} `{}`, and '{name}' expects `{}` — not stored",
            mismatch.claim, mismatch.found, mismatch.expected
        )));
    }
    userconfig::save_registry_token(name, token)?;
    Ok(RegistryLoginOutput {
        name: name.to_string(),
    })
}

/// `vaire registry logout <name>`. Not an error to log out of a registry you never
/// logged in to — nor of one no longer configured, which is why this does not `find`
/// the row: forgetting a token is never the wrong thing to allow.
pub fn logout(name: &str) -> Result<RegistryLogoutOutput> {
    let name = name.trim();
    if name.is_empty() {
        return Err(VaireError::Usage("a registry needs a name".into()));
    }
    Ok(RegistryLogoutOutput {
        name: name.to_string(),
        forgotten: userconfig::forget_registry_token(name)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::wire::{Auth, Capabilities, Descriptor};

    fn descriptor(auth: Option<Auth>) -> Descriptor {
        Descriptor {
            schema_version: 1,
            name: None,
            capabilities: Capabilities::default(),
            auth,
        }
    }

    fn identity() -> Option<Auth> {
        Some(Auth {
            issuer: "https://login.example/tenant".into(),
            audience: None,
            client_id: None,
            scopes: vec![],
            grants: vec![],
        })
    }

    #[test]
    fn identity_over_plain_http_is_refused_except_on_loopback() {
        let refused = insecure_identity("http://packages.example/kg", &descriptor(identity()));
        assert!(refused.is_some_and(|why| why.contains("https://")));

        for local in [
            "http://localhost:8080",
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
        ] {
            assert!(
                insecure_identity(local, &descriptor(identity())).is_none(),
                "{local} has nothing on the wire to eavesdrop on"
            );
        }
        assert!(
            insecure_identity("https://packages.example/kg", &descriptor(identity())).is_none()
        );
        // Anonymous over http is the whole static-host story and stays allowed.
        assert!(insecure_identity("http://packages.example/kg", &descriptor(None)).is_none());
        assert!(insecure_identity("file:///srv/registry", &descriptor(identity())).is_none());
        // An api-tier registry takes a token on every write, issuer declared or not.
        let mut api = descriptor(None);
        api.capabilities.publish = Some(PublishCapability::Api);
        assert!(insecure_identity("http://packages.example/kg", &api).is_some());
        assert!(insecure_identity("http://127.0.0.1:8080", &api).is_none());
    }

    #[test]
    fn two_names_that_share_a_token_variable_cannot_both_be_configured() {
        let home = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let url = format!("file://{}", store.path().display());
        add(home.path(), "prod.eu", &url, 0, true).expect("first name records");
        let err = add(home.path(), "prod-eu", &url, 0, true).unwrap_err();
        assert!(
            err.to_string().contains("VAIRE_TOKEN_PROD_EU"),
            "names the colliding variable: {err}"
        );
        // Re-adding the same name is an update, not a collision with itself.
        add(home.path(), "prod.eu", &url, 5, true).expect("same name re-adds");
    }

    #[test]
    fn a_registry_takes_identity_when_it_names_an_issuer_or_publishes_through_the_api() {
        assert!(
            !takes_identity(&descriptor(None)),
            "a static host is anonymous"
        );
        assert!(takes_identity(&descriptor(identity())));
        let mut api = descriptor(None);
        api.capabilities.publish = Some(PublishCapability::Api);
        assert!(
            takes_identity(&api),
            "api-tier writes are never anonymous, whoever issued the token"
        );
    }
}
