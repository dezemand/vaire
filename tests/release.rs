//! `vaire release` — the classifier and the release ritual (cli.md §4.7).
//!
//! Every test drives the real command against a throwaway Git corpus, because the whole
//! feature *is* its interaction with git: the baseline comes from a tag, the evidence
//! from a diff of two committed trees, and the result is a commit and a tag.

mod common;

use common::{Corpus, git};
use vaire::commands::release::{self, Options};
use vaire::error::VaireError;
use vaire::output::ReleaseStatus;

/// A corpus that indexes everything (what every real package's globs do) so release
/// records land inside the corpus.
fn corpus() -> Corpus {
    let c = Corpus::empty();
    std::fs::write(
        c.root().join("knowledge.toml"),
        "name = \"acme-core\"\nversion = \"1.0.0\"\ninclude = [\"**/*.md\"]\n\
         types = [\"person\", \"department\", \"method\", \"system\", \"record\", \"project\"]\n",
    )
    .unwrap();
    c.add(
        "knowledge/platform.md",
        "---\nid: platform\ntype: department\nname: Platform\n---\n# Platform\n\nThe platform group.\n",
    )
    .add(
        "knowledge/jane.md",
        "---\nid: jane-doe\ntype: person\nname: Jane Doe\norg: department:platform\n---\n# Jane Doe\n\nWorks in [[department:platform]].\n",
    )
    .commit()
    .build();
    c
}

fn release(c: &Corpus, options: Options<'_>) -> vaire::output::ReleaseOutput {
    release::run(&c.ctx(), options).expect("release runs")
}

/// Cut the first release, so later tests start from a real baseline.
fn released_once(c: &Corpus) -> vaire::output::ReleaseOutput {
    release(c, Options::default())
}

#[test]
fn a_first_release_publishes_the_declared_version_verbatim() {
    let c = corpus();
    let out = released_once(&c);

    assert_eq!(out.status, ReleaseStatus::Released);
    // Nothing to diff means nothing to increment: a first release is a declaration.
    assert_eq!(out.version, "1.0.0", "{out:?}");
    assert_eq!(out.bump, None);
    assert_eq!(out.tag.as_deref(), Some("v1.0.0"));
    assert!(
        c.root().join("releases/1-0-0.md").is_file(),
        "the release record is written into the corpus"
    );
    // The record ships *inside* the release it describes, not one commit behind it.
    let files = vaire::git::list_files_at_head(c.root()).unwrap();
    assert!(
        files.contains(&"releases/1-0-0.md".to_string()),
        "{files:?}"
    );
    assert_eq!(
        vaire::git::resolve_rev(c.root(), "v1.0.0").unwrap(),
        vaire::git::head(c.root()).unwrap(),
        "the tag names the release commit"
    );
}

/// A first release records what it publishes — everything the corpus holds.
///
/// There is nothing to diff, but that is not the same as nothing to say. The record's
/// edges are how "which release published this?" is answered, and no later release
/// re-adds an entity that was already there — so an empty first record would leave every
/// founding entity permanently unattributed, in the one release where they are all of
/// them. Release records are still excluded, on the same grounds the diff excludes them.
#[test]
fn a_first_release_records_everything_it_publishes() {
    let c = corpus();
    let out = released_once(&c);

    assert_eq!(
        out.classification.added,
        vec!["department:platform".to_string(), "person:jane-doe".to_string()],
        "the founding entities are what 1.0.0 added"
    );
    assert!(
        out.classification.changed.is_empty()
            && out.classification.retired.is_empty()
            && out.classification.removed.is_empty()
    );

    let record = std::fs::read_to_string(c.root().join("releases/1-0-0.md")).unwrap();
    assert!(
        record.contains("[[department:platform]]") && record.contains("[[person:jane-doe]]"),
        "the record links what it published:\n{record}"
    );

    // And the point of the edges: the query the design sells actually answers.
    let ctx = c.ctx();
    let index = ctx.open_index().expect("index");
    let releases: Vec<String> = index
        .query_rows(
            "SELECT from_id FROM edges WHERE to_id = ?1 AND to_package IS NULL",
            ["department:platform"],
            |r| Ok(r.get_value(0)?.as_text().cloned().unwrap_or_default()),
        )
        .expect("backlinks");
    assert!(
        releases.iter().any(|id| id == "release:1-0-0"),
        "the founding entity is attributed to the release that published it: {releases:?}"
    );
}

#[test]
fn adding_an_entity_is_a_minor() {
    let c = corpus();
    released_once(&c);
    c.add(
        "knowledge/ingest.md",
        "---\nid: ingest\ntype: system\nname: Ingest\n---\n# Ingest\n\nEvent ingestion.\n",
    )
    .commit();

    let out = release(&c, Options::default());
    assert_eq!(out.version, "1.1.0", "{out:?}");
    assert_eq!(out.classification.added, ["system:ingest"]);
    assert!(out.classification.changed.is_empty(), "{out:?}");
}

#[test]
fn editing_prose_is_a_patch() {
    let c = corpus();
    released_once(&c);
    c.add(
        "knowledge/platform.md",
        "---\nid: platform\ntype: department\nname: Platform\n---\n# Platform\n\nThe platform group, which owns the build system.\n",
    )
    .commit();

    let out = release(&c, Options::default());
    assert_eq!(out.version, "1.0.1", "{out:?}");
    assert_eq!(out.classification.changed, ["department:platform"]);
}

#[test]
fn a_changed_edge_is_a_change_but_a_touched_date_is_not() {
    let c = corpus();
    released_once(&c);
    // Re-pointing an edge is a change to the graph consumers read...
    c.add(
        "knowledge/jane.md",
        "---\nid: jane-doe\ntype: person\nname: Jane Doe\norg: department:logistics\n---\n# Jane Doe\n\nWorks in [[department:platform]].\n",
    )
    .add(
        "knowledge/logistics.md",
        "---\nid: logistics\ntype: department\nname: Logistics\n---\n# Logistics\n\nThe logistics group.\n",
    )
    .commit();
    let out = release(&c, Options::default());
    assert!(
        out.classification
            .changed
            .contains(&"person:jane-doe".to_string()),
        "a re-pointed edge is a change: {out:?}"
    );

    // ...while bookkeeping is not. `updated:` is metadata about the file, not something
    // a reader of the entity notices.
    c.add(
        "knowledge/jane.md",
        "---\nid: jane-doe\ntype: person\nname: Jane Doe\norg: department:logistics\nupdated: 2026-08-11\n---\n# Jane Doe\n\nWorks in [[department:platform]].\n",
    )
    .commit();
    let out = release(&c, Options::default());
    assert_eq!(
        out.status,
        ReleaseStatus::Nothing,
        "a touched `updated:` alone must not manufacture a release: {out:?}"
    );
}

#[test]
fn removing_an_entity_blocks_until_a_human_says_major() {
    let c = corpus();
    released_once(&c);
    std::fs::remove_file(c.root().join("knowledge/jane.md")).unwrap();
    c.commit();

    let blocked = release(&c, Options::default());
    assert_eq!(blocked.status, ReleaseStatus::Blocked, "{blocked:?}");
    assert_eq!(blocked.classification.removed, ["person:jane-doe"]);
    assert!(
        vaire::git::resolve_rev(c.root(), "v2.0.0")
            .unwrap()
            .is_none(),
        "a blocked release writes nothing"
    );

    // A major must say what it invalidates before it may proceed.
    let err = release::run(
        &c.ctx(),
        Options {
            major: true,
            ..Default::default()
        },
    )
    .expect_err("major without notes is refused");
    assert!(matches!(err, VaireError::Release(_)), "{err:?}");

    let notes = c.root().join("notes.md");
    std::fs::write(
        &notes,
        "`person:jane-doe` was removed; use the department instead.\n",
    )
    .unwrap();
    let out = release(
        &c,
        Options {
            major: true,
            notes: Some(&notes),
            ..Default::default()
        },
    );
    assert_eq!(out.version, "2.0.0", "{out:?}");
    let record = std::fs::read_to_string(c.root().join("releases/2-0-0.md")).unwrap();
    assert!(record.contains("## Invalidated assumptions"), "{record}");
    // A removed entity has no address left to link to, so the record names it as text —
    // a reference would be a dangling edge that fails the *next* release's gate.
    assert!(record.contains("- `person:jane-doe`"), "{record}");
    assert!(!record.contains("[[person:jane-doe]]"), "{record}");
}

#[test]
fn a_tombstone_retires_rather_than_removes_and_stays_linkable() {
    let c = corpus();
    released_once(&c);
    c.add(
        "knowledge/jane.md",
        "---\nid: jane-doe\ntype: person\nname: Jane Doe\nsuperseded_by: department:platform\n---\n# Jane Doe\n",
    )
    .commit();

    let blocked = release(&c, Options::default());
    assert_eq!(blocked.status, ReleaseStatus::Blocked);
    assert_eq!(blocked.classification.retired, ["person:jane-doe"]);
    assert!(
        blocked.classification.removed.is_empty(),
        "a tombstoned entity still exists: {blocked:?}"
    );

    let notes = c.root().join("notes.md");
    std::fs::write(&notes, "Jane's entity now redirects to the department.\n").unwrap();
    let out = release(
        &c,
        Options {
            major: true,
            notes: Some(&notes),
            ..Default::default()
        },
    );
    let record = std::fs::read_to_string(c.root().join("releases/2-0-0.md")).unwrap();
    assert!(
        record.contains("[[person:jane-doe]]"),
        "a retired entity is still addressable, so the record links it: {record}"
    );
    assert_eq!(out.version, "2.0.0");
}

#[test]
fn major_may_be_forced_for_a_meaning_change_the_classifier_cannot_see() {
    let c = corpus();
    released_once(&c);
    // One word, reversing what the entity asserts. Structurally this is a PATCH.
    c.add(
        "knowledge/platform.md",
        "---\nid: platform\ntype: department\nname: Platform\n---\n# Platform\n\nThe platform group no longer owns deployment.\n",
    )
    .commit();
    assert_eq!(
        release(
            &c,
            Options {
                dry_run: true,
                ..Default::default()
            }
        )
        .version,
        "1.0.1",
        "the classifier only sees structure"
    );

    let notes = c.root().join("notes.md");
    std::fs::write(
        &notes,
        "Ownership of deployment moved; prior guidance is void.\n",
    )
    .unwrap();
    let out = release(
        &c,
        Options {
            major: true,
            notes: Some(&notes),
            ..Default::default()
        },
    );
    assert_eq!(out.version, "2.0.0", "the maintainer owns meaning: {out:?}");
}

#[test]
fn release_records_are_excluded_from_the_classifier() {
    let c = corpus();
    released_once(&c);
    // The trap this guards: every release writes a record, so a classifier that counted
    // records would see a new entity every time and no release could ever be a PATCH —
    // nor could "nothing changed" ever be true again.
    let out = release(&c, Options::default());
    assert_eq!(out.status, ReleaseStatus::Nothing, "{out:?}");
    assert!(
        out.classification.added.is_empty(),
        "the 1.0.0 record must be invisible to classification: {out:?}"
    );
}

#[test]
fn nothing_to_release_is_a_clean_no_op_not_a_failure() {
    let c = corpus();
    released_once(&c);
    // An automated pipeline runs this on every merge; most merges warrant no version.
    c.add("README.md", "# Not corpus\n").commit();

    let out = release(&c, Options::default());
    assert_eq!(out.status, ReleaseStatus::Nothing);
    assert!(
        vaire::git::resolve_rev(c.root(), "v1.0.1")
            .unwrap()
            .is_none(),
        "no tag, no commit, exit 0"
    );
}

#[test]
fn a_dry_run_writes_nothing() {
    let c = corpus();
    released_once(&c);
    c.add(
        "knowledge/ingest.md",
        "---\nid: ingest\ntype: system\nname: Ingest\n---\n# Ingest\n\nEvent ingestion.\n",
    )
    .commit();
    let head_before = vaire::git::head(c.root()).unwrap();

    let out = release(
        &c,
        Options {
            dry_run: true,
            ..Default::default()
        },
    );
    assert_eq!(out.status, ReleaseStatus::Planned);
    assert_eq!(out.version, "1.1.0");
    assert_eq!(
        vaire::git::head(c.root()).unwrap(),
        head_before,
        "no commit"
    );
    assert!(!c.root().join("releases/1-1-0.md").exists(), "no record");
    assert!(
        vaire::git::resolve_rev(c.root(), "v1.1.0")
            .unwrap()
            .is_none()
    );
    let manifest = std::fs::read_to_string(c.root().join("knowledge.toml")).unwrap();
    assert!(manifest.contains("version = \"1.0.0\""), "{manifest}");
}

#[test]
fn a_dirty_tree_is_refused() {
    let c = corpus();
    released_once(&c);
    c.add(
        "knowledge/scratch.md",
        "---\nid: wip\ntype: system\n---\n# WIP\n",
    );

    let err = release::run(&c.ctx(), Options::default()).expect_err("dirty tree refused");
    match err {
        VaireError::Release(msg) => assert!(msg.contains("uncommitted"), "{msg}"),
        other => panic!("expected a release refusal, got {other:?}"),
    }
}

#[test]
fn releasing_off_the_mainline_is_refused_unless_allowed() {
    let c = corpus();
    released_once(&c);
    git(c.root(), &["checkout", "-q", "-b", "topic"]);
    c.add(
        "knowledge/ingest.md",
        "---\nid: ingest\ntype: system\nname: Ingest\n---\n# Ingest\n\nEvent ingestion.\n",
    )
    .commit();

    let err = release::run(&c.ctx(), Options::default()).expect_err("off-branch refused");
    match err {
        // A tag cut here would name a commit the mainline may never contain.
        VaireError::Release(msg) => assert!(msg.contains("--allow-branch"), "{msg}"),
        other => panic!("expected a release refusal, got {other:?}"),
    }

    let out = release(
        &c,
        Options {
            allow_branch: true,
            ..Default::default()
        },
    );
    assert_eq!(out.version, "1.1.0", "the escape hatch works: {out:?}");
}

#[test]
fn the_manifest_version_is_rewritten_without_disturbing_the_file() {
    let c = Corpus::empty();
    std::fs::write(
        c.root().join("knowledge.toml"),
        "# acme-core — the shared vocabulary\n\
         name = \"acme-core\"\nversion = \"1.0.0\"\ninclude = [\"**/*.md\"]\n\n\
         # types this package defines\ntypes = [\"system\"]\n",
    )
    .unwrap();
    c.add(
        "knowledge/ingest.md",
        "---\nid: ingest\ntype: system\nname: Ingest\n---\n# Ingest\n\nEvent ingestion.\n",
    )
    .commit()
    .build();
    released_once(&c);
    c.add(
        "knowledge/billing.md",
        "---\nid: billing\ntype: system\nname: Billing\n---\n# Billing\n\nInvoices.\n",
    )
    .commit();
    release(&c, Options::default());

    let manifest = std::fs::read_to_string(c.root().join("knowledge.toml")).unwrap();
    assert!(manifest.contains("version = \"1.1.0\""), "{manifest}");
    assert!(
        manifest.contains("# acme-core — the shared vocabulary")
            && manifest.contains("# types this package defines"),
        "comments survive a tool-managed field being rewritten: {manifest}"
    );
}

#[test]
fn a_record_the_globs_would_not_index_is_refused_before_anything_is_written() {
    let c = Corpus::empty();
    // Narrow globs that do not select `releases/` — writing a record here would leave a
    // file describing a release that nothing could ever query.
    std::fs::write(
        c.root().join("knowledge.toml"),
        "name = \"acme-core\"\nversion = \"1.0.0\"\ninclude = [\"knowledge/**/*.md\"]\n\
         types = [\"system\"]\n",
    )
    .unwrap();
    c.add(
        "knowledge/ingest.md",
        "---\nid: ingest\ntype: system\nname: Ingest\n---\n# Ingest\n\nEvent ingestion.\n",
    )
    .commit()
    .build();

    let err = release::run(&c.ctx(), Options::default()).expect_err("refused");
    match err {
        VaireError::Release(msg) => assert!(msg.contains("include"), "names the fix: {msg}"),
        other => panic!("expected a release refusal, got {other:?}"),
    }
    assert!(!c.root().join("releases").exists(), "nothing written");
}

#[test]
fn a_hand_cut_tag_becomes_the_baseline() {
    let c = corpus();
    released_once(&c);
    // Someone released 1.1.0 by hand — the tag records what was published, so it is what
    // the next release counts from, whatever the manifest happens to say.
    git(c.root(), &["tag", "-a", "-m", "manual", "v1.1.0"]);
    c.add(
        "knowledge/ingest.md",
        "---\nid: ingest\ntype: system\nname: Ingest\n---\n# Ingest\n\nEvent ingestion.\n",
    )
    .commit();

    let out = release(&c, Options::default());
    assert_eq!(out.version, "1.2.0", "{out:?}");
    assert_eq!(
        vaire::release::latest_release(c.root(), "acme-core")
            .unwrap()
            .map(|(t, _)| t),
        Some("v1.2.0".to_string())
    );
}

#[test]
fn the_baseline_is_the_highest_version_not_the_newest_tag() {
    let c = corpus();
    released_once(&c);
    // A 1.9.0 released, then a 1.10.0 — whose tag is newer but whose name sorts lower
    // as text. Picking the wrong baseline here would diff against the wrong tree.
    for (n, id) in [(9u32, "nine"), (10, "ten")] {
        c.add(
            &format!("knowledge/{id}.md"),
            &format!("---\nid: {id}\ntype: system\nname: {id}\n---\n# {id}\n\nA system.\n"),
        )
        .commit();
        git(c.root(), &["tag", "-a", "-m", "x", &format!("v1.{n}.0")]);
    }
    c.add(
        "knowledge/last.md",
        "---\nid: last\ntype: system\nname: Last\n---\n# Last\n\nA system.\n",
    )
    .commit();

    let out = release(
        &c,
        Options {
            dry_run: true,
            ..Default::default()
        },
    );
    assert_eq!(
        out.version, "1.11.0",
        "1.10.0 is the baseline, not 1.9.0: {out:?}"
    );
}

#[test]
fn a_patch_to_heavily_cited_material_is_flagged_but_not_blocked() {
    let c = corpus();
    // Eleven entities all pointing at one — over the "worth a second look" threshold.
    for n in 0..11 {
        c.add(
            &format!("knowledge/dep{n}.md"),
            &format!(
                "---\nid: dep{n}\ntype: system\nname: Dep {n}\nowner: department:platform\n---\n# Dep {n}\n\nOwned by [[department:platform]].\n"
            ),
        );
    }
    c.commit();
    released_once(&c);

    c.add(
        "knowledge/platform.md",
        "---\nid: platform\ntype: department\nname: Platform\n---\n# Platform\n\nThe platform group, reorganised.\n",
    )
    .commit();

    // `--yes` is the CI posture: the advisory is reported, never prompted, never fatal.
    let out = release(
        &c,
        Options {
            yes: true,
            ..Default::default()
        },
    );
    assert_eq!(out.version, "1.0.1", "still a patch: {out:?}");
    assert_eq!(out.advisories.len(), 1, "{out:?}");
    assert_eq!(out.advisories[0].id, "department:platform");
    assert!(out.advisories[0].inbound >= 11, "{out:?}");
}

#[test]
fn an_uncited_edit_carries_no_advisory() {
    let c = corpus();
    released_once(&c);
    c.add(
        "knowledge/jane.md",
        "---\nid: jane-doe\ntype: person\nname: Jane Doe\norg: department:platform\n---\n# Jane Doe\n\nWorks in [[department:platform]]. Now on the build team.\n",
    )
    .commit();

    let out = release(
        &c,
        Options {
            yes: true,
            ..Default::default()
        },
    );
    assert!(out.advisories.is_empty(), "{out:?}");
}

#[test]
fn a_dry_run_predicts_the_gate_it_would_hit() {
    let c = corpus();
    released_once(&c);
    c.add(
        "knowledge/broken.md",
        "---\nid: broken\ntype: system\nname: Broken\nowner: department:does-not-exist\n---\n# Broken\n\nPoints at [[department:does-not-exist]].\n",
    )
    .commit();

    // A merge-request pipeline asks --dry-run what merging will do. If it answered
    // "would release 1.1.0" while the real release refused, the reviewer would be told
    // the opposite of the truth.
    let err = release::run(
        &c.ctx(),
        Options {
            dry_run: true,
            ..Default::default()
        },
    )
    .expect_err("the dry run reports the gate");
    assert!(matches!(err, VaireError::CheckViolations(_)), "{err:?}");
}

#[test]
fn a_release_leaves_the_index_current_and_the_record_queryable() {
    let c = corpus();
    released_once(&c);

    // The release commit lands after the index was built, so without folding it in the
    // package would be left one commit stale — and the record it just wrote invisible to
    // every read command until someone ran `vaire index` by hand.
    let status = vaire::commands::status::run(&c.ctx()).expect("status");
    assert_eq!(status.commits_behind_head, 0, "{status:?}");
    let pending = status.pending_release.expect("status reports releases");
    assert_eq!(pending.would_be, "none", "{pending:?}");

    let resolved = vaire::commands::resolve::run(&c.ctx(), "release:1-0-0")
        .expect("the record is queryable straight after the release");
    assert_eq!(resolved.node_type, "release");
}

#[test]
fn major_cannot_escalate_a_first_release() {
    let c = corpus();
    let notes = c.root().join("notes.md");
    std::fs::write(
        &notes,
        "Nothing to invalidate; there is no prior version.\n",
    )
    .unwrap();

    // A major is a claim *relative to* a previous release. With no prior tag there is
    // nothing to be major against, so `--major` must not turn a manifest's 1.0.0 into
    // 2.0.0 — the first release publishes what the manifest declares, full stop.
    let out = release(
        &c,
        Options {
            major: true,
            notes: Some(&notes),
            ..Default::default()
        },
    );
    assert_eq!(out.version, "1.0.0", "{out:?}");
    assert_eq!(out.bump, None, "{out:?}");
    assert_eq!(out.tag.as_deref(), Some("v1.0.0"));
}

#[test]
fn renaming_the_release_type_does_not_manufacture_a_major() {
    let c = corpus();
    released_once(&c);

    // The baseline is parsed with the manifest committed at its tag, so each side must be
    // told its own release type. Otherwise the old records go unexcluded on the baseline
    // side, present as removed, and force a spurious MAJOR on a package that only renamed
    // a vocabulary word.
    let manifest = std::fs::read_to_string(c.root().join("knowledge.toml")).unwrap();
    std::fs::write(
        c.root().join("knowledge.toml"),
        manifest.replace(
            "types = [",
            "release_type = \"changelog\"\nrelease_dir = \"changelogs\"\ntypes = [",
        ),
    )
    .unwrap();
    c.commit();

    let out = release(&c, Options::default());
    assert!(
        out.classification.removed.is_empty(),
        "the old records must stay excluded: {out:?}"
    );
    assert_ne!(out.bump, Some(vaire::model::Bump::Major), "{out:?}");
}

#[test]
fn an_unreadable_tag_listing_is_an_error_not_an_empty_history() {
    // "No tags" means "never released", which skips the baseline diff and republishes the
    // manifest version. A git failure must therefore never masquerade as an empty tag list.
    let dir = tempfile::tempdir().unwrap();
    assert!(
        vaire::git::tags(dir.path()).is_err(),
        "a non-repository must error rather than report no tags"
    );
}

// ---------------------------------------------------------------------------------------
// `--summary` — prose from outside, checked before it becomes history (cli.md §4.7)
// ---------------------------------------------------------------------------------------

/// Write a summary file *inside* the corpus root — the harder of the two cases, and the one
/// CI produces, since a build artifact lands in the checkout. (Outside the root is covered
/// by `a_summary_outside_the_corpus_is_read_the_same_way`.)
fn summary_file(c: &Corpus, text: &str) -> std::path::PathBuf {
    let path = c.root().join("release-summary.md");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn a_summary_becomes_a_section_and_its_references_become_edges() {
    let c = corpus();
    released_once(&c);
    c.add(
        "knowledge/ingest.md",
        "---\nid: ingest\ntype: system\nname: Ingest\n---\n# Ingest\n\nEvent ingestion.\n",
    )
    .commit();

    let summary = summary_file(
        &c,
        "Ingestion moved in-house: [[system:ingest]] now owns what \
         [[department:platform]] used to do by hand.\n",
    );
    let out = release(
        &c,
        Options {
            summary: Some(&summary),
            ..Default::default()
        },
    );
    assert_eq!(out.version, "1.1.0", "{out:?}");
    assert!(out.summary, "the output reports that prose rode along");

    let record = std::fs::read_to_string(c.root().join("releases/1-1-0.md")).unwrap();
    assert!(record.contains("## Summary"), "{record}");
    assert!(record.contains("generated_summary: true"), "{record}");
    assert!(record.contains("[[system:ingest]] now owns"), "{record}");
    // The computed lists are untouched by the prose.
    assert!(record.contains("added: [system:ingest]"), "{record}");

    // The summary's references are ordinary edges: a release record that mentions an
    // entity is discoverable from that entity, which is the whole reason records exist.
    let refs = vaire::commands::refs::run(&c.ctx(), "release:1-1-0", 1, None).unwrap();
    let targets: Vec<&str> = refs.refs.iter().map(|r| r.id.as_str()).collect();
    assert!(targets.contains(&"department:platform"), "{targets:?}");

    // The release commit carries the manifest and the record, and nothing else — the
    // summary file itself is an input, not release content.
    let files = vaire::git::list_files_at_head(c.root()).unwrap();
    assert!(
        files.contains(&"releases/1-1-0.md".to_string()),
        "{files:?}"
    );
    assert!(
        !files.contains(&"release-summary.md".to_string()),
        "the summary file is an input, never committed: {files:?}"
    );
}

#[test]
fn an_address_the_summary_imagined_refuses_the_release_and_leaves_no_trace() {
    let c = corpus();
    released_once(&c);
    c.add(
        "knowledge/ingest.md",
        "---\nid: ingest\ntype: system\nname: Ingest\n---\n# Ingest\n\nEvent ingestion.\n",
    )
    .commit();
    let before = vaire::git::head(c.root()).unwrap();

    let summary = summary_file(&c, "Routing now flows through [[system:getway]].\n");
    let err = release::run(
        &c.ctx(),
        Options {
            summary: Some(&summary),
            ..Default::default()
        },
    )
    .expect_err("a reference nothing can resolve must refuse the release");

    let message = err.to_string();
    assert!(message.contains("system:getway"), "{message}");
    assert!(message.contains("releases/1-1-0.md"), "{message}");

    // Nothing committed, nothing tagged, and the record it was about to write is gone —
    // the tree is exactly where it started, so a fixed summary can simply be re-run.
    assert_eq!(vaire::git::head(c.root()).unwrap(), before);
    assert!(
        vaire::git::resolve_rev(c.root(), "v1.1.0")
            .unwrap()
            .is_none(),
        "a refused release leaves no tag"
    );
    assert!(
        !c.root().join("releases/1-1-0.md").exists(),
        "the record is rolled back"
    );
    // The manifest never moved, so the retry computes the same version.
    let manifest = std::fs::read_to_string(c.root().join("knowledge.toml")).unwrap();
    assert!(manifest.contains("version = \"1.0.0\""), "{manifest}");

    // And the retry with a real address succeeds where the first attempt refused.
    let summary = summary_file(&c, "Routing now flows through [[system:ingest]].\n");
    let out = release(
        &c,
        Options {
            summary: Some(&summary),
            ..Default::default()
        },
    );
    assert_eq!(out.version, "1.1.0", "{out:?}");
}

#[test]
fn a_dry_run_rehearses_the_summary_gate_and_writes_nothing() {
    let c = corpus();
    released_once(&c);
    c.add(
        "knowledge/ingest.md",
        "---\nid: ingest\ntype: system\nname: Ingest\n---\n# Ingest\n\nEvent ingestion.\n",
    )
    .commit();

    // A dry run's job is to predict the real run, including the one gate only a written
    // record can answer.
    let bad = summary_file(&c, "Routing flows through [[system:getway]].\n");
    let err = release::run(
        &c.ctx(),
        Options {
            dry_run: true,
            summary: Some(&bad),
            ..Default::default()
        },
    )
    .expect_err("the rehearsal must refuse what the real run would refuse");
    assert!(err.to_string().contains("system:getway"), "{err:?}");
    assert!(
        !c.root().join("releases/1-1-0.md").exists(),
        "a dry run leaves nothing behind, even when it wrote to check"
    );

    let good = summary_file(&c, "Ingestion moved in-house: [[system:ingest]].\n");
    let out = release(
        &c,
        Options {
            dry_run: true,
            summary: Some(&good),
            ..Default::default()
        },
    );
    assert_eq!(out.status, ReleaseStatus::Planned);
    assert!(out.summary, "{out:?}");
    assert!(
        !c.root().join("releases/1-1-0.md").exists(),
        "a dry run writes nothing"
    );
    assert!(
        vaire::git::resolve_rev(c.root(), "v1.1.0")
            .unwrap()
            .is_none(),
        "a dry run tags nothing"
    );
}

#[test]
fn a_summary_may_retitle_the_record_but_never_restate_the_classification() {
    let c = corpus();
    released_once(&c);
    c.add(
        "knowledge/ingest.md",
        "---\nid: ingest\ntype: system\nname: Ingest\n---\n# Ingest\n\nEvent ingestion.\n",
    )
    .commit();

    // The keys the classifier owns are refused by name — never silently dropped, or the
    // author believes they described a release they did not.
    let claimed = summary_file(
        &c,
        "---\nadded: [system:nothing]\nbump: major\n---\nProse.\n",
    );
    let err = release::run(
        &c.ctx(),
        Options {
            summary: Some(&claimed),
            ..Default::default()
        },
    )
    .expect_err("a summary cannot restate what the classifier computed");
    let message = err.to_string();
    assert!(message.contains("added"), "{message}");
    assert!(message.contains("bump"), "{message}");

    // A title and keys of the author's own are theirs to set.
    let titled = summary_file(
        &c,
        "---\nname: \"Ingestion in-house\"\nsummary_by: an agent\n---\nIngestion moved.\n",
    );
    let out = release(
        &c,
        Options {
            summary: Some(&titled),
            ..Default::default()
        },
    );
    assert_eq!(out.version, "1.1.0", "{out:?}");
    let record = std::fs::read_to_string(c.root().join("releases/1-1-0.md")).unwrap();
    assert!(record.contains("name: \"Ingestion in-house\""), "{record}");
    assert!(record.contains("summary_by: an agent"), "{record}");
    // The version spellings survive the retitle: they are how the record is found.
    assert!(
        record.contains("aliases: [\"1.1.0\", \"v1.1.0\"]"),
        "{record}"
    );
    assert!(record.contains("id: 1-1-0"), "{record}");

    // …so the record still answers to its address, and the version spellings still find it
    // even though the title no longer mentions a version at all.
    assert!(
        vaire::commands::resolve::run(&c.ctx(), "release:1-1-0").is_ok(),
        "a retitled record keeps its address"
    );
    let found = vaire::commands::suggest::run(&c.ctx(), "1.1.0", None, None, true).unwrap();
    assert!(
        found
            .suggestions
            .iter()
            .any(|candidate| candidate.id == "release:1-1-0"),
        "the dotted version must still find the record: {found:?}"
    );
}

#[test]
fn a_dry_run_reports_the_notes_a_major_owes_instead_of_refusing_over_them() {
    let c = corpus();
    released_once(&c);
    std::fs::remove_file(c.root().join("knowledge/jane.md")).unwrap();
    c.add(
        "knowledge/platform.md",
        "---\nid: platform\ntype: department\nname: Platform\n---\n# Platform\n\nThe platform group.\n",
    )
    .commit();

    // The plan an agent needs in order to draft the very notes the real run demands. A dry
    // run writes nothing, so reporting the debt is a faithful prediction, not a loosened gate.
    let out = release(
        &c,
        Options {
            major: true,
            dry_run: true,
            ..Default::default()
        },
    );
    assert_eq!(out.status, ReleaseStatus::Planned);
    assert_eq!(out.version, "2.0.0", "{out:?}");
    assert!(out.notes_required, "{out:?}");
    assert_eq!(out.classification.removed, ["person:jane-doe"]);

    // The real run still refuses.
    let err = release::run(
        &c.ctx(),
        Options {
            major: true,
            ..Default::default()
        },
    )
    .expect_err("the real run still demands the notes");
    assert!(matches!(err, VaireError::Release(_)), "{err:?}");
}

#[test]
fn the_input_files_are_not_corpus_the_release_is_judged_on() {
    let c = corpus();
    released_once(&c);
    c.add(
        "knowledge/jane.md",
        "---\nid: jane-doe\ntype: person\nname: Jane Doe\nsuperseded_by: department:platform\n---\n# Jane Doe\n",
    )
    .commit();

    // A maintainer's scratch notes file, sitting in the checkout, that happens to carry
    // `id:`/`type:` frontmatter — so the working-tree pass indexes it as a node — and a
    // reference of its own that resolves to nothing. It is exempt from the release commit;
    // it must be exempt from the release's checks for the same reason, or the release is
    // refused over content nobody is publishing.
    let notes = c.root().join("release-notes.md");
    std::fs::write(
        &notes,
        "---\nid: scratch-notes\ntype: department\nrelated: department:nonexistent\n---\n\
         Jane's entity now redirects to the department.\n",
    )
    .unwrap();
    let summary = summary_file(
        &c,
        "Jane's entity was retired in favour of the department.\n",
    );

    let out = release(
        &c,
        Options {
            major: true,
            notes: Some(&notes),
            summary: Some(&summary),
            ..Default::default()
        },
    );
    assert_eq!(out.version, "2.0.0", "{out:?}");
    assert!(out.summary, "{out:?}");

    // Neither input is in the release commit, and neither refused it.
    let files = vaire::git::list_files_at_head(c.root()).unwrap();
    for input in ["release-notes.md", "release-summary.md"] {
        assert!(
            !files.contains(&input.to_string()),
            "{input} is an input, never release content: {files:?}"
        );
    }
}

#[test]
fn a_summary_outside_the_corpus_is_read_the_same_way() {
    let c = corpus();
    released_once(&c);
    c.add(
        "knowledge/ingest.md",
        "---\nid: ingest\ntype: system\nname: Ingest\n---\n# Ingest\n\nEvent ingestion.\n",
    )
    .commit();

    // A path outside the root is not in the tree at all, so it needs no exemption from
    // either gate — the release must read it exactly as it reads one inside.
    let outside = tempfile::tempdir().unwrap();
    let summary = outside.path().join("release-summary.md");
    std::fs::write(&summary, "Ingestion moved in-house: [[system:ingest]].\n").unwrap();

    let out = release(
        &c,
        Options {
            summary: Some(&summary),
            ..Default::default()
        },
    );
    assert_eq!(out.version, "1.1.0", "{out:?}");
    let record = std::fs::read_to_string(c.root().join("releases/1-1-0.md")).unwrap();
    assert!(record.contains("Ingestion moved in-house"), "{record}");
}

/// Deleting a released entity must not brick the package.
///
/// A release record links what that release published, so a later hard deletion leaves an
/// edge pointing at an address that no longer resolves. Treated as an ordinary dangling
/// reference it would be unfixable — records are immutable — and permanent, and because
/// `release` gates on `check`, the package could never be released again. The record is a
/// statement about the past, and the past is allowed to mention what is gone.
#[test]
fn a_record_may_cite_an_entity_a_later_release_removed() {
    let c = corpus();
    released_once(&c);
    std::fs::remove_file(c.root().join("knowledge/jane.md")).unwrap();
    c.commit();

    let notes = c.root().join("notes.md");
    std::fs::write(&notes, "`person:jane-doe` is gone.\n").unwrap();
    let out = release(
        &c,
        Options {
            major: true,
            notes: Some(&notes),
            yes: true,
            ..Default::default()
        },
    );
    assert_eq!(out.status, ReleaseStatus::Released, "{out:?}");
    assert_eq!(out.version, "2.0.0");

    // 1.0.0's record still links person:jane-doe, which no longer exists — and check is
    // clean, because that edge is the classifier's own record of history.
    let record = std::fs::read_to_string(c.root().join("releases/1-0-0.md")).unwrap();
    assert!(record.contains("[[person:jane-doe]]"), "{record}");
    let (report, failed) = vaire::commands::check::run(&c.ctx(), false, false, false).unwrap();
    assert!(
        !failed,
        "a removed entity a record cites is history, not a broken reference: {:?}",
        report.violations
    );

    // And the exemption is narrow: an address nothing published is still dangling, so a
    // hand-written reference in a record is checked like any other.
    std::fs::write(
        c.root().join("releases/1-0-0.md"),
        format!("{record}\nSee also [[person:nobody]].\n"),
    )
    .unwrap();
    c.commit().build();
    let (report, failed) = vaire::commands::check::run(&c.ctx(), false, false, false).unwrap();
    assert!(
        failed
            && report.violations.iter().any(|v| matches!(
                v,
                vaire::index::check::Violation::DanglingRef { to, .. } if to == "person:nobody"
            )),
        "an invented address in a record is still a violation: {:?}",
        report.violations
    );
}
