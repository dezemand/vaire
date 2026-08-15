//! `vaire push` (cli.md §4.10) — uploading released versions to a registry
//! (registry.v2.md §3.4).
//!
//! **Split from `release` on purpose**, and the split is the interesting part. Cutting a
//! version is a git act: it computes a bump, writes a record, commits, and tags. Uploading
//! is idempotent plumbing. Keeping them apart buys two things that matter more than the
//! convenience of one command:
//!
//! * A failed upload must not re-run a ritual. `push` is safe to run again, and again, and
//!   the second run is a no-op for everything already published.
//! * **CI must be able to publish a tag it did not cut.** That is the whole shape of the
//!   package workflow (amendment 16): a maintainer merges, the pipeline releases, and some
//!   other pipeline — later, elsewhere, on a fresh clone — publishes.
//!
//! ## Tags are the source, not `.vaire/dist/`
//!
//! `push` enumerates this package's release tags and rebuilds each artifact from the tag's
//! own tree (amendment 15). Nothing has to have been packed beforehand, which is what lets
//! it work in a container that cloned the repository thirty seconds ago. That is only sound
//! because `pack` is deterministic: the artifact rebuilt from `v1.4.2` today is byte-for-byte
//! the one that tag would have produced when it was cut, so its checksum — the thing a
//! lockfile pins — is a property of the release rather than of the machine that uploaded it.
//!
//! ## What "already published" means
//!
//! The registry is asked, and its answer is authoritative. A version it lists is skipped
//! silently; a version whose artifact upload is refused by storage was published by someone
//! else between the question and the write, and is reported rather than overwritten. Neither
//! is a failure, because both leave the registry holding exactly what it should.

use std::path::Path;

use crate::commands::Ctx;
use crate::corpus::repo::Repo;
use crate::error::{Result, VaireError};
use crate::model::Version;
use crate::output::{PushOutput, PushedRelease};
use crate::registry::wire::{Access, AccessEnforcement};
use crate::registry::{PublishRequest, Registry, RegistryError};

pub struct Options<'a> {
    /// Publish only this version. Default: every release tag the registry does not have.
    pub version: Option<&'a str>,
    pub registry: Option<&'a str>,
    /// `open` | `restricted` | `unlisted`. `None` leaves whatever the registry already
    /// records — access is sticky (§8.5).
    pub access: Option<&'a str>,
    pub access_hint: Option<&'a str>,
    /// Report what would be published; upload nothing.
    pub dry_run: bool,
}

pub fn run(ctx: &Ctx, options: Options<'_>) -> Result<PushOutput> {
    let root = ctx.repo.root();
    let package = ctx.config.name.clone();

    // The same constraint `pack` and `release` carry: a package nested in a larger
    // repository cannot name a commit that is only its own, so it has no release tags of
    // its own to publish.
    if !ctx.repo.is_git_root() {
        return Err(VaireError::Registry(format!(
            "`vaire push` publishes this package's release tags, and {} is not a Git \
             repository of its own",
            root.display()
        )));
    }

    let access = match options.access {
        None => None,
        Some(state) => Some(
            Access::parse(state, options.access_hint.map(str::to_string)).ok_or_else(|| {
                VaireError::Usage(format!(
                    "--access takes one of {}; got '{state}'",
                    Access::STATES.join(", ")
                ))
            })?,
        ),
    };
    if options.access.is_none() && options.access_hint.is_some() {
        return Err(VaireError::Usage(
            "--access-hint says where to ask for a package that is not pullable, so it \
             only means something with `--access restricted`"
                .into(),
        ));
    }

    let row = crate::commands::registry::select(ctx.home(), options.registry)?;
    let registry = crate::commands::registry::open(&row)?;
    let mut warnings = Vec::new();

    if registry.descriptor().capabilities.publish.is_none() {
        return Err(VaireError::Registry(format!(
            "registry '{}' at {} cannot be published to from here",
            row.name, row.url
        )));
    }
    // A courtesy flag must never be mistaken for a control (§8.5). Said at the moment the
    // flag is set, which is the only moment the person setting it is thinking about it.
    if access.as_ref().is_some_and(|a| !a.pullable)
        && registry.descriptor().capabilities.access_enforcement == AccessEnforcement::Advisory
    {
        warnings.push(format!(
            "registry '{}' enforces access advisorily: `--access restricted` records intent \
             and routes people to you, but anything the host serves, its readers can fetch",
            row.name
        ));
    }

    // ---- what there is to publish ------------------------------------------------------
    let tagged = release_tags(root, &package)?;
    if tagged.is_empty() {
        return Err(VaireError::Registry(format!(
            "{package} has no release tags — `vaire release` cuts one, and `push` uploads \
             what it cut"
        )));
    }
    let wanted = match options.version {
        None => tagged.clone(),
        Some(requested) => {
            let version: Version = requested.parse().map_err(|_| {
                VaireError::Usage(format!("'{requested}' is not a MAJOR.MINOR.PATCH version"))
            })?;
            let found = tagged
                .iter()
                .find(|(_, v)| *v == version)
                .cloned()
                .ok_or_else(|| {
                    VaireError::Registry(format!(
                        "{package} {version} is not tagged in this repository; \
                         tagged: {}",
                        versions_list(&tagged)
                    ))
                })?;
            vec![found]
        }
    };

    // The registry's own answer about what it holds. `NotFound` here means the package has
    // never been published, which is the ordinary first-push case rather than a problem.
    let published: Vec<Version> = match registry.versions(&package) {
        Ok(releases) => releases.into_iter().map(|r| r.version).collect(),
        Err(RegistryError::NotFound { .. }) => Vec::new(),
        Err(e) => return Err(e.into()),
    };

    let mut out = PushOutput {
        package: package.clone(),
        registry: row.name.clone(),
        url: row.url.clone(),
        published: Vec::new(),
        already: Vec::new(),
        failed: Vec::new(),
        dry_run: options.dry_run,
        warnings,
    };

    let dist = Repo::prepare_derived_dir(root)?.join("dist");
    // An explicitly named version bypasses the skip below: `vaire push 1.4.2` means "make
    // sure 1.4.2 is published", so it packs and attempts, and the digest check in
    // `publish_one` then actually confirms that what the registry holds is this release.
    // That is the command to reach for when a conflict is suspected.
    let named = options.version.is_some();
    for (tag, version) in &wanted {
        // The listing is trusted here, and this is the one place it is. Verifying a
        // historical version would mean re-packing it — a second on one tag, a minute on
        // fifty — on every push, which would cost the idempotence that makes `push` safe to
        // re-run far more than it would buy. The version being published *now* is verified,
        // because there the digest is already in hand.
        if !named && published.contains(version) {
            out.already.push(version.to_string());
            continue;
        }
        if options.dry_run {
            out.published.push(PushedRelease {
                version: version.to_string(),
                sha256: String::new(),
                size_bytes: 0,
                url: String::new(),
            });
            continue;
        }
        // One version's failure is reported and stepped over: publishing is per-release,
        // and a tag from two years ago with a broken relative link must not stop today's
        // release from reaching the registry.
        match publish_one(
            registry.as_ref(),
            root,
            &dist,
            tag,
            *version,
            prior_of(&tagged, *version),
            access.clone(),
        ) {
            Ok(Outcome::Published(release, notes)) => {
                out.published.push(release);
                out.warnings.extend(notes);
            }
            // Someone else published it between the preflight question and our write, and
            // the bytes match. The registry holds the immutable release either way, which
            // is the outcome we wanted — so it belongs with what was already there.
            Ok(Outcome::AlreadyPublished) => out.already.push(version.to_string()),
            Err(e) => out.failed.push(crate::output::PushFailure {
                version: version.to_string(),
                reason: e.to_string(),
            }),
        }
    }
    Ok(out)
}

/// What publishing one version came to.
enum Outcome {
    Published(PushedRelease, Vec<String>),
    /// The registry already had it, **and its bytes are this release's bytes** — the
    /// preflight missed it, or a concurrent publisher won the create-only write. Not a
    /// failure: the registry holds exactly what this push was trying to put there.
    AlreadyPublished,
}

/// Build one tag's artifact and publish it.
fn publish_one(
    registry: &dyn Registry,
    root: &Path,
    dist: &Path,
    tag: &str,
    version: Version,
    prior_version: Option<Version>,
    access: Option<Access>,
) -> Result<Outcome> {
    let artifact = crate::commands::pack::at_rev(root, tag, dist)?;
    // The manifest at the tag has to agree with the tag, or the artifact would be published
    // under a version its own manifest does not claim.
    if artifact.version != version.to_string() {
        return Err(VaireError::Registry(format!(
            "{tag} declares version {} in its knowledge.toml — the tag and the manifest \
             disagree, so there is no honest version to publish this as",
            artifact.version
        )));
    }
    let changelog = changelog_at(root, tag, version)?;
    let excerpt = changelog.as_deref().and_then(invalidated_assumptions);

    let published = match registry.publish(PublishRequest {
        name: &artifact.name,
        version,
        artifact: &artifact.path,
        changelog: changelog.as_deref(),
        changelog_excerpt: excerpt.as_deref(),
        deps: artifact.dependencies.clone(),
        description: artifact.description.as_deref(),
        access,
        // Reserved for a `validate_bump`-capable registry: what the publisher claims, so
        // the claim can be checked rather than trusted. Static hosts ignore both.
        claimed_bump: None,
        prior_version,
    }) {
        Ok(published) => published,
        // Storage refused the create-only write, which says only that *something* occupies
        // this (name, version) — not that it is this release. Those are very different
        // situations and reporting the second as the first would be the worst outcome
        // available: a push that says "already published" while the intended release never
        // reached the registry and never will, because the identity is immutably taken.
        //
        // The digest settles it, and costs nothing: the artifact was just built.
        Err(RegistryError::VersionExists { .. }) => {
            return match published_digest(registry, &artifact.name, version) {
                Some(theirs) if theirs == artifact.sha256 => Ok(Outcome::AlreadyPublished),
                Some(theirs) => Err(VaireError::Registry(format!(
                    "{} {version} is already published, and it is not this release — the \
                     registry serves {}, this tag packs to {}. A published version is \
                     immutable, so this cannot be corrected by pushing; release a new \
                     version, or find out whose {version} that is",
                    artifact.name,
                    short(&theirs),
                    short(&artifact.sha256),
                ))),
                // The artifact is there and the index does not describe it — a half-written
                // publish, or an index edited by hand. Either way this push cannot claim the
                // version and cannot confirm it either.
                None => Err(VaireError::Registry(format!(
                    "{} {version} is already published, but the registry's index does not \
                     record it — the registry is in an inconsistent state",
                    artifact.name
                ))),
            };
        }
        Err(e) => return Err(e.into()),
    };

    let mut warnings = artifact.warnings;
    warnings.extend(published.warnings);
    Ok(Outcome::Published(
        PushedRelease {
            version: published.version.to_string(),
            sha256: published.sha256,
            size_bytes: published.size,
            url: published.artifact_url,
        },
        warnings,
    ))
}

/// The digest the registry records for a published version, if it records one.
fn published_digest(registry: &dyn Registry, name: &str, version: Version) -> Option<String> {
    registry
        .versions(name)
        .ok()?
        .into_iter()
        .find(|release| release.version == version)
        .map(|release| release.sha256)
}

fn short(digest: &str) -> &str {
    &digest[..digest.len().min(12)]
}

/// Every release tag of `package`, lowest version first.
fn release_tags(root: &Path, package: &str) -> Result<Vec<(String, Version)>> {
    let mut tagged: Vec<(String, Version)> = crate::git::tags(root)?
        .into_iter()
        .filter_map(|tag| {
            let version = crate::release::parse_release_tag(&tag, package)?;
            Some((tag, version))
        })
        .collect();
    // Oldest first, so a first push of a long history writes the index document in the
    // order the releases happened — and so an interrupted push resumes at the right place.
    tagged.sort_by_key(|(_, version)| *version);
    Ok(tagged)
}

/// The version released immediately before `version`, for a registry that validates bumps.
fn prior_of(tagged: &[(String, Version)], version: Version) -> Option<Version> {
    tagged
        .iter()
        .map(|(_, v)| *v)
        .filter(|v| *v < version)
        .max()
}

/// The release record committed at `tag`, which is what the wire serves as this version's
/// changelog (amendment 20 — the record is the source, so `CHANGELOG.md` is not a second
/// one).
///
/// `None` when the package released before records existed, or opted out. Missing history
/// is not an error: an old tag is not going to grow a record, and refusing to publish it
/// would make the tool's own past a blocker.
fn changelog_at(root: &Path, tag: &str, version: Version) -> Result<Option<String>> {
    let manifest = crate::git::show_many_at(root, tag, &["knowledge.toml".to_string()])?
        .into_iter()
        .next()
        .flatten();
    let Some(manifest) = manifest else {
        return Ok(None);
    };
    let config = crate::config::Config::parse(&manifest, &format!("{tag}:knowledge.toml"))?;
    let rel = crate::release::record::path_for(&config, version)
        .to_string_lossy()
        .replace('\\', "/");
    Ok(crate::git::show_many_at(root, tag, &[rel])?
        .into_iter()
        .next()
        .flatten())
}

/// The `## Invalidated assumptions` section of a release record.
///
/// This is the one piece of a changelog that has to be readable **without downloading
/// anything** (§8.3): a dependent deciding whether to adopt a major is deciding whether its
/// own references still hold, and making it fetch an artifact to find that out would put the
/// cost in exactly the wrong place.
fn invalidated_assumptions(changelog: &str) -> Option<String> {
    let start = changelog
        .lines()
        .position(|line| line.trim_start_matches('#').trim() == "Invalidated assumptions")?;
    let body: Vec<&str> = changelog
        .lines()
        .skip(start + 1)
        .take_while(|line| !line.starts_with("## "))
        .collect();
    let text = body.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn versions_list(tagged: &[(String, Version)]) -> String {
    tagged
        .iter()
        .map(|(_, v)| v.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_major_excerpt_stops_at_the_next_section() {
        let record = "# 2.0.0\n\nReleased 2026-08-14 — major — 3 removed.\n\n\
                      ## Invalidated assumptions\n\n\
                      `concept:torque-vectoring` was split in two; anything citing it must\n\
                      re-confirm which half it meant.\n\n\
                      ## Added\n\n- [[concept:a]]\n";
        let excerpt = invalidated_assumptions(record).unwrap();
        assert!(excerpt.starts_with("`concept:torque-vectoring` was split"));
        assert!(
            !excerpt.contains("Added"),
            "the next section is not part of it"
        );
    }

    #[test]
    fn a_minor_release_record_has_no_excerpt() {
        let record = "# 1.5.0\n\nReleased 2026-08-14 — minor — 2 new.\n\n## Added\n\n- [[a:b]]\n";
        assert_eq!(invalidated_assumptions(record), None);
    }

    #[test]
    fn the_prior_version_is_the_highest_below_it_not_the_previous_tag_alphabetically() {
        let tagged = vec![
            ("v1.9.0".to_string(), Version::new(1, 9, 0)),
            ("v1.10.0".to_string(), Version::new(1, 10, 0)),
            ("v2.0.0".to_string(), Version::new(2, 0, 0)),
        ];
        assert_eq!(
            prior_of(&tagged, Version::new(2, 0, 0)),
            Some(Version::new(1, 10, 0))
        );
        assert_eq!(prior_of(&tagged, Version::new(1, 9, 0)), None);
    }
}
