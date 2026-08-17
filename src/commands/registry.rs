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

use std::path::Path;

use crate::catalog::{Catalog, KIND_STATIC, RegistryRow};
use crate::error::{Result, VaireError};
use crate::output::{
    RegistryAddOutput, RegistryListOutput, RegistryRemoveOutput, RegistryShowOutput,
};
use crate::registry::{Registry, StaticHttp, transport};

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
    {
        let catalog = Catalog::open(home)?;
        catalog.add_registry(&row)?;
    }

    // Probe after recording, so a failed probe cannot lose the configuration the user just
    // asked for. What comes back is reported as information, never as a verdict.
    let probe = match StaticHttp::open(name, &url) {
        Ok(registry) => {
            let writable = registry.descriptor().capabilities.publish.is_some();
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
    Ok(RegistryShowOutput {
        name: row.name,
        url: row.url,
        priority: row.priority,
        search_by_default: row.search_by_default,
        schema_version: descriptor.schema_version,
        declared_name: descriptor.name,
        capabilities: descriptor.capabilities,
        packages,
    })
}

/// The configured registry called `name`.
pub fn find(home: &Path, name: &str) -> Result<RegistryRow> {
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
        other => Err(VaireError::Registry(format!(
            "registry '{}' is recorded as kind '{other}', which this vaire cannot speak — \
             re-add it, or run `vaire upgrade`",
            row.name
        ))),
    }
}
