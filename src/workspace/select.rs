//! Choosing which package on this machine satisfies a declared dependency
//! (registry.v2.md §6).
//!
//! v0.2.0 answered "is there something called this?" by walking a configured root, which
//! could only match on a name. The catalog answers the same question as a lookup, and the
//! extra column changes what is answerable: a `^MAJOR` constraint stops being a lint
//! reported after the fact and becomes the **selector**. Two clones of `acme-core` at 1.4
//! and 2.0 are no longer an ambiguity — a consumer declaring `^1` wants exactly one of
//! them, and it says so.
//!
//! ## The catalog is an index, never truth
//!
//! Every row is an observation, and the manifest beside it is the fact. So a candidate is
//! **re-read before it is believed**: one file, right before anything is linked. What that
//! read finds is written back, because a working copy is *supposed* to change under you —
//! a version bumped by a release, a package renamed mid-refactor. The row catches up
//! instead of going stale, which is why nothing here ever needs a scan to heal.
//!
//! ## Ambiguity is refused, not tiebroken
//!
//! Once the constraint has filtered, two remaining candidates are two working copies of
//! the same major — a fork beside its original, or two worktrees on different branches.
//! Picking the higher version would be a guess dressed as arithmetic, and a fork is
//! routinely newer than what it forked from. One tier does apply first: a path someone
//! explicitly ran `vaire catalog add` on outranks one a scan or a passing command noticed,
//! because that is a statement of intent and it gives ambiguity a resolution that is not
//! "edit every consumer's links".

use std::path::PathBuf;

use crate::catalog::{Catalog, Origin, State};
use crate::config::Config;
use crate::error::Result;
use crate::model::Version;

/// A package the catalog knows about, as its manifest describes it *now*.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    /// The version its manifest declares, verbatim — kept even when it does not parse, so
    /// a report can show what was actually written there.
    pub version: String,
    /// The parsed form, `None` when `version` is not a `MAJOR.MINOR.PATCH` triple.
    pub parsed: Option<Version>,
    pub origin: Origin,
}

impl Candidate {
    fn satisfies(&self, constraint: &str) -> bool {
        self.parsed
            .is_some_and(|version| version.satisfies_caret(constraint))
    }
}

/// What the catalog had to say about one declared dependency.
#[derive(Debug)]
pub enum Selection {
    /// Exactly one candidate satisfies the constraint.
    One(Candidate),
    /// Nothing on this machine declares the name.
    Unknown,
    /// Packages declaring the name exist, but none is in the constrained major line.
    Unsatisfied(Vec<Candidate>),
    /// Several satisfy, and choosing between them would be a guess.
    Ambiguous(Vec<Candidate>),
}

/// Select the local package satisfying `name` at `constraint`.
///
/// Rows are verified against their manifests as they are considered, and the catalog is
/// updated with what the verification found — a moved-away path is marked `missing`, a
/// changed version or name is recorded. That write-back is the self-healing: the next
/// lookup starts from what is true now.
pub fn select(catalog: &Catalog, name: &str, constraint: &str) -> Result<Selection> {
    let mut candidates = Vec::new();
    for sighting in catalog.by_name(name)? {
        let manifest = sighting.path.join("knowledge.toml");
        // `is_file` first, and deliberately: `Config::load` answers a *missing* manifest
        // with defaults (the standalone-corpus case), so loading alone would turn a
        // deleted checkout into a nameless package that silently matches nothing.
        let loaded = manifest.is_file().then(|| Config::load(&manifest));
        let Some(Ok(config)) = loaded else {
            // Gone, or no longer a package. Either way it cannot satisfy anything, and the
            // row should stop claiming it can. (An unreadable manifest is deliberately
            // treated the same: what matters here is that nothing can be resolved from it.)
            if sighting.state != State::Missing {
                let _ = catalog.set_state(&sighting.path, State::Missing);
            }
            continue;
        };
        // Believe the manifest over the row, and write back what it said. A package that
        // now declares a different name is not a candidate for this one — but it is still
        // a package, so the row follows it rather than being deleted.
        if config.name != name || config.version != sighting.version {
            let _ = catalog.record(
                &sighting.path,
                &config.name,
                &config.version,
                sighting.origin,
            );
        } else if sighting.state != State::Live {
            // Unchanged, but it answered — so the row saying `missing` is now wrong.
            // Demoting without promoting would let `catalog rm --missing` delete the row
            // for a package this very selection is about to link.
            let _ = catalog.set_state(&sighting.path, State::Live);
        }
        if config.name != name {
            continue;
        }
        candidates.push(Candidate {
            path: sighting.path,
            parsed: config.version.parse().ok(),
            version: config.version,
            origin: sighting.origin,
        });
    }

    if candidates.is_empty() {
        return Ok(Selection::Unknown);
    }
    let satisfying: Vec<Candidate> = candidates
        .iter()
        .filter(|c| c.satisfies(constraint))
        .cloned()
        .collect();
    match satisfying.len() {
        0 => Ok(Selection::Unsatisfied(candidates)),
        1 => Ok(Selection::One(satisfying.into_iter().next().expect("one"))),
        _ => Ok(prefer_registered(satisfying)),
    }
}

/// An explicit `vaire catalog add` outranks a path a scan or a passing command noticed.
/// Only that one tier exists: within it, ambiguity is reported.
fn prefer_registered(satisfying: Vec<Candidate>) -> Selection {
    let registered: Vec<Candidate> = satisfying
        .iter()
        .filter(|c| c.origin == Origin::Registered)
        .cloned()
        .collect();
    let short_list = match registered.len() {
        1 => return Selection::One(registered.into_iter().next().expect("one")),
        0 => satisfying,
        _ => registered,
    };
    Selection::Ambiguous(short_list)
}

/// The note recorded against a dependency the catalog could not settle — the same text
/// whether it surfaces through `index`'s warning rows or `check`'s missing-dependency
/// violation, so one explanation exists for one situation.
pub fn explain(name: &str, constraint: &str, selection: &Selection) -> Option<String> {
    match selection {
        Selection::One(_) => None,
        Selection::Unknown => Some(format!(
            "no package declaring '{name}' is in your catalog — record one with \
             `vaire catalog add <path>`, or import a tree of them with `vaire catalog scan <dir>`"
        )),
        Selection::Unsatisfied(found) => {
            let seen: Vec<String> = found
                .iter()
                .map(|c| format!("{} at {}", c.version, c.path.display()))
                .collect();
            Some(format!(
                "no package declaring '{name}' satisfies {constraint} — this machine has {}",
                seen.join(", ")
            ))
        }
        Selection::Ambiguous(candidates) => {
            let paths: Vec<String> = candidates
                .iter()
                .map(|c| c.path.display().to_string())
                .collect();
            Some(format!(
                "'{name}' {constraint} is satisfied by more than one package here ({}) — \
                 declare which with `vaire catalog add <path>`, or link one explicitly",
                paths.join(", ")
            ))
        }
    }
}
