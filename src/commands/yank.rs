//! `vaire yank <name>@<version> [--undo]` (cli.md §4.11) — marking a published release as
//! one nobody should newly adopt (registry.v2.md §8.4).
//!
//! A yank is **an edit to the index document, and nothing else**. The artifact stays exactly
//! where it was, byte for byte, which is the entire difference between this and a deletion:
//! a lockfile pinning that version keeps resolving, a build that already depends on it keeps
//! building, and what changes is only what a *new* resolution will choose.
//!
//! That is also why it is reversible. `--undo` clears the flag, because the reason for a
//! yank is often a mistake about the release rather than a fact about it.
//!
//! Corpus-independent: yanking is something you do to a registry, and the package whose
//! release is being withdrawn need not be — and after a bad publish, often is not — the
//! directory you happen to be standing in.

use std::path::Path;

use crate::error::{Result, VaireError};
use crate::model::Version;
use crate::output::YankOutput;
use crate::registry::wire::checked_name;

/// `spec` is `<name>@<version>`.
pub fn run(home: &Path, spec: &str, registry: Option<&str>, undo: bool) -> Result<YankOutput> {
    let (name, version) = parse_spec(spec)?;
    let row = crate::commands::registry::select(home, registry)?;
    let client = crate::commands::registry::open(&row)?;
    if !client.descriptor().capabilities.yank {
        return Err(VaireError::Registry(format!(
            "registry '{}' does not support yanking",
            row.name
        )));
    }
    client.yank(&name, version, !undo)?;
    Ok(YankOutput {
        package: name,
        version: version.to_string(),
        registry: row.name,
        yanked: !undo,
    })
}

/// `acme-core@1.4.2` → `("acme-core", 1.4.2)`.
///
/// The `@` form rather than two positional arguments, because a yank names one exact
/// release and the pair should be inseparable in the shell history that records it.
fn parse_spec(spec: &str) -> Result<(String, Version)> {
    let usage = || {
        VaireError::Usage(format!(
            "`vaire yank` takes <name>@<version>, e.g. `acme-core@1.4.2`; got '{spec}'"
        ))
    };
    let (name, version) = spec.rsplit_once('@').ok_or_else(usage)?;
    checked_name(name).map_err(VaireError::Usage)?;
    let version: Version = version.parse().map_err(|_| usage())?;
    Ok((name.to_string(), version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_is_name_at_version() {
        let (name, version) = parse_spec("acme-core@1.4.2").unwrap();
        assert_eq!(name, "acme-core");
        assert_eq!(version, Version::new(1, 4, 2));
    }

    #[test]
    fn a_half_written_spec_is_a_usage_error_not_a_guess() {
        for bad in [
            "acme-core",
            "acme-core@",
            "@1.4.2",
            "acme-core@1.4",
            "acme-core@v1.4.2",
        ] {
            assert!(parse_spec(bad).is_err(), "{bad} should not parse");
        }
    }
}
