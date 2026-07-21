//! Spec tests for the maintain commands (cli.md §4). Red until `index::build`,
//! `index::check`, and `commands::status` are implemented (step 2 / step 3).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{Corpus, CountingEmbedder, head};

use vaire::commands;
use vaire::index::build::Mode;
use vaire::index::check::{Violation, Warning};

// ---- index + status (cli.md §4.1, §4.3) ------------------------------------

#[test]
fn status_reports_commit_and_counts_after_build() {
    let c = Corpus::fixture();
    let out = commands::status::run(&c.ctx()).unwrap();

    // Indexing is commit-bound: the index records exactly HEAD, with nothing behind.
    assert_eq!(
        out.last_indexed_commit.as_deref(),
        Some(head(c.root()).as_str())
    );
    assert_eq!(out.commits_behind_head, 0);

    // The fixture has 9 nodes (6 entities + 1 project + 2 records).
    assert_eq!(out.nodes.total, 9);
    assert_eq!(out.nodes.by_type.get("person"), Some(&2));
    assert_eq!(out.nodes.by_type.get("record"), Some(&2));
    assert!(out.edges > 0);
}

#[test]
fn status_behind_head_after_new_commit() {
    let c = Corpus::fixture();
    // A new commit the index has not yet absorbed.
    c.add(
        "knowledge/entities/methods/cqrs.md",
        "---\nid: cqrs\ntype: method\nname: CQRS\n---\n# CQRS\n",
    )
    .commit();

    let out = commands::status::run(&c.ctx()).unwrap();
    assert_eq!(out.commits_behind_head, 1);
}

// ---- check (cli.md §4.2) ---------------------------------------------------

#[test]
fn check_clean_fixture_has_no_violations() {
    let c = Corpus::fixture();
    let (report, failed) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    assert!(!failed);
}

#[test]
fn check_detects_duplicate_id() {
    let c = Corpus::empty();
    c.add(
        "knowledge/a.md",
        "---\nid: dup\ntype: person\nname: A\n---\n# A\nlinks [[department:x]]\n",
    )
    .add(
        "knowledge/b.md",
        "---\nid: dup\ntype: person\nname: B\n---\n# B\nlinks [[department:x]]\n",
    )
    .add(
        "knowledge/x.md",
        "---\nid: x\ntype: department\nname: X\n---\n# X\n",
    )
    .commit()
    .build();

    let (report, failed) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(failed);
    assert!(
        report
            .violations
            .iter()
            .any(|v| matches!(v, Violation::DuplicateId { id, .. } if id == "person:dup"))
    );
}

#[test]
fn check_detects_dangling_reference() {
    let c = Corpus::empty();
    c.add(
        "knowledge/r.md",
        "---\nid: r\ntype: record\nreferences: [system:ghost]\n---\n# R\n",
    )
    .commit()
    .build();

    let (report, failed) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(failed);
    assert!(
        report
            .violations
            .iter()
            .any(|v| matches!(v, Violation::DanglingRef { to, .. } if to == "system:ghost"))
    );
}

#[test]
fn orphan_is_a_warning_not_a_failure_unless_strict() {
    let c = Corpus::empty();
    // A lone node with no inbound or outbound edges.
    c.add(
        "knowledge/lonely.md",
        "---\nid: lonely\ntype: method\nname: Lonely\n---\n# Lonely\n",
    )
    .commit()
    .build();

    let (report, failed) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::Orphan { id, .. } if id == "method:lonely"))
    );
    assert!(!failed, "orphans are warnings by default");

    // --strict promotes the warning to a failure (exit 6).
    let (_report, failed_strict) = commands::check::run(&c.ctx(), true, false).unwrap();
    assert!(failed_strict);
}

#[test]
fn check_flags_undeclared_cross_package_import() {
    // A cross-package `@pkg/` reference whose package isn't in [dependencies] is an
    // undeclared import — a violation (packages.md §8). Pure table check, no resolution.
    let c = Corpus::empty();
    c.add(
        "knowledge/checkout.md",
        "---\nid: checkout\ntype: system\nowner: \"@acme-core/department:platform\"\n---\n# Checkout\n",
    )
    .commit()
    .build();

    let (report, failed) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(failed, "undeclared import is a violation");
    assert!(
        report.violations.iter().any(|v| matches!(
            v,
            Violation::UndeclaredImport { package, to, .. }
                if package == "acme-core" && to == "@acme-core/department:platform"
        )),
        "expected undeclared_import: {:?}",
        report.violations
    );
    // It is NOT reported as a local dangling reference.
    assert!(
        !report
            .violations
            .iter()
            .any(|v| matches!(v, Violation::DanglingRef { .. }))
    );
}

#[test]
fn declared_cross_package_import_is_clean() {
    // Same reference, but the package is declared → no undeclared_import. It still doesn't
    // resolve (that's M5), but declaring the dependency clears the manifest-only lint.
    let c = Corpus::empty();
    c.with_dependencies(&[("acme-core", "^1")]).add(
        "knowledge/checkout.md",
        "---\nid: checkout\ntype: system\nowner: \"@acme-core/department:platform\"\n---\n# Checkout\n",
    )
    .commit()
    .build();

    let (report, _) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(
        !report
            .violations
            .iter()
            .any(|v| matches!(v, Violation::UndeclaredImport { .. })),
        "declared dependency should clear the lint: {:?}",
        report.violations
    );
}

#[test]
fn check_warns_on_unreferenceable_declared_id() {
    // The file indexes (files are truth), but its declared id falls outside the strict
    // reference grammar (design.md §6), so no reference can ever address it — the trap
    // is surfaced as a warning, never silent, never a block.
    let c = Corpus::empty();
    c.add(
        "knowledge/jane.md",
        "---\nid: Jane_Doe\ntype: person\nname: Jane\n---\n# Jane\n",
    )
    .commit()
    .build();

    let (report, failed) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::UnreferenceableId { id, .. } if id == "person:Jane_Doe")),
        "expected unreferenceable_id: {:?}",
        report.warnings
    );
    assert!(!failed, "a warning, not a violation");

    // The node itself still indexed — files are truth.
    let status = commands::status::run(&c.ctx()).unwrap();
    assert_eq!(status.nodes.total, 1);
}

#[test]
fn drift_is_an_advisory_warning_for_inline_only_refs() {
    let c = Corpus::empty();
    c.add("knowledge/x.md", "---\nid: x\ntype: system\nname: X\n---\n# X\n")
        .add("knowledge/y.md", "---\nid: y\ntype: system\nname: Y\n---\n# Y\n")
        .add(
            "knowledge/r.md",
            // system:y is declared in frontmatter; system:x is only mentioned inline.
            "---\nid: r\ntype: record\nreferences: [system:y]\n---\n# R\n\nMentions [[system:x]] and [[system:y]].\n",
        )
        .commit()
        .build();

    let (report, failed) = commands::check::run(&c.ctx(), false, false).unwrap();
    // Inline-only system:x drifts; system:y (also in frontmatter) does not.
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::Drift { to, .. } if to == "system:x"))
    );
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::Drift { to, .. } if to == "system:y"))
    );
    assert!(report.violations.is_empty());
    assert!(!failed, "drift is advisory");

    // --strict promotes it to a failure.
    let (_r, failed_strict) = commands::check::run(&c.ctx(), true, false).unwrap();
    assert!(failed_strict);
}

#[test]
fn incremental_reindex_reembeds_only_changed_sections() {
    let c = Corpus::empty();
    c.add(
        "knowledge/n.md",
        "---\nid: n\ntype: method\nname: N\n---\n# N\n\n## Alpha\n\nalpha body\n\n## Beta\n\nbeta body\n",
    )
    .commit();

    let counter = Arc::new(AtomicUsize::new(0));
    let emb = CountingEmbedder {
        dims: 8,
        embedded: counter.clone(),
    };

    // Full build embeds all three sections (preamble + Alpha + Beta).
    c.build_with(&emb, Mode::Full);
    assert!(counter.swap(0, Ordering::Relaxed) >= 3);

    // Change only the Beta section, then reindex incrementally.
    c.add(
        "knowledge/n.md",
        "---\nid: n\ntype: method\nname: N\n---\n# N\n\n## Alpha\n\nalpha body\n\n## Beta\n\nbeta body CHANGED\n",
    )
    .commit();
    c.build_with(&emb, Mode::Incremental);

    // Preamble + Alpha are content-hash cache hits; only Beta is re-embedded.
    assert_eq!(counter.load(Ordering::Relaxed), 1);
}

#[test]
fn re_embed_bypasses_the_cache_and_reembeds_all_sections() {
    let c = Corpus::empty();
    c.add(
        "knowledge/n.md",
        "---\nid: n\ntype: method\nname: N\n---\n# N\n\n## Alpha\n\nalpha\n\n## Beta\n\nbeta\n",
    )
    .commit();

    let counter = Arc::new(AtomicUsize::new(0));
    let emb = CountingEmbedder {
        dims: 8,
        embedded: counter.clone(),
    };

    // Full build embeds all three sections and populates the content-hash cache.
    c.build_with(&emb, Mode::Full);
    assert_eq!(counter.swap(0, Ordering::Relaxed), 3);

    // --re-embed ignores the cache and re-embeds every section (a plain reindex with no
    // changes would embed 0 — all cache hits).
    c.reembed_with(&emb);
    assert_eq!(counter.load(Ordering::Relaxed), 3);
}

// ---- working-tree indexing (cli.md §4.1 --working-tree) --------------------

fn node_name(c: &Corpus, id: &str) -> String {
    commands::resolve::run(&c.ctx(), id).unwrap().frontmatter["name"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn working_tree_index_sees_uncommitted_edits() {
    let c = Corpus::empty();
    c.add(
        "knowledge/x.md",
        "---\nid: x\ntype: method\nname: Original\n---\n# X\n",
    )
    .commit()
    .build(); // committed index → Original
    assert_eq!(node_name(&c, "method:x"), "Original");

    // Edit the file but do NOT commit.
    c.add(
        "knowledge/x.md",
        "---\nid: x\ntype: method\nname: Edited\n---\n# X\n",
    );

    // A committed reindex ignores the uncommitted edit.
    c.build_with(&common::DummyEmbedder { dims: 8 }, Mode::Full);
    assert_eq!(node_name(&c, "method:x"), "Original");

    // A working-tree reindex picks it up.
    c.build_with(&common::DummyEmbedder { dims: 8 }, Mode::WorkingTree);
    assert_eq!(node_name(&c, "method:x"), "Edited");

    // The working-tree index records no commit (it isn't a commit snapshot).
    assert_eq!(
        commands::status::run(&c.ctx()).unwrap().last_indexed_commit,
        None
    );
}

#[test]
fn plain_index_restores_committed_state_after_working_tree() {
    let c = Corpus::empty();
    c.add(
        "knowledge/x.md",
        "---\nid: x\ntype: method\nname: Committed\n---\n# X\n",
    )
    .commit()
    .build();

    // Uncommitted edit, indexed with --working-tree.
    c.add(
        "knowledge/x.md",
        "---\nid: x\ntype: method\nname: WorkingTreeOnly\n---\n# X\n",
    );
    c.build_with(&common::DummyEmbedder { dims: 8 }, Mode::WorkingTree);
    assert_eq!(node_name(&c, "method:x"), "WorkingTreeOnly");
    assert_eq!(
        commands::status::run(&c.ctx()).unwrap().last_indexed_commit,
        None
    );

    // A plain `vaire index` (Mode::Incremental, the default) must restore the committed
    // state — dropping the uncommitted edit and re-anchoring to HEAD.
    c.build_with(&common::DummyEmbedder { dims: 8 }, Mode::Incremental);
    assert_eq!(node_name(&c, "method:x"), "Committed");
    assert_eq!(
        commands::status::run(&c.ctx())
            .unwrap()
            .last_indexed_commit
            .as_deref(),
        Some(head(c.root()).as_str())
    );
}

#[test]
fn status_distinguishes_working_tree_from_not_built() {
    let c = Corpus::empty();
    c.add(
        "knowledge/x.md",
        "---\nid: x\ntype: method\nname: X\n---\n# X\n",
    )
    .commit();
    c.build_with(&common::DummyEmbedder { dims: 8 }, Mode::WorkingTree);

    let out = commands::status::run(&c.ctx()).unwrap();
    // Built from the working tree: a real index (nodes present), but no commit — and the
    // source says so, rather than looking like "not built yet".
    assert_eq!(out.source.as_deref(), Some("working-tree"));
    assert_eq!(out.last_indexed_commit, None);
    assert!(out.nodes.total >= 1);
}

#[test]
fn check_working_tree_validates_uncommitted_edits() {
    let c = Corpus::empty();
    c.add(
        "knowledge/r.md",
        "---\nid: r\ntype: record\nreferences: [system:ok]\n---\n# R\n",
    )
    .add(
        "knowledge/ok.md",
        "---\nid: ok\ntype: system\nname: OK\n---\n# OK\n",
    )
    .commit()
    .build();

    // Introduce a dangling reference in an uncommitted edit.
    c.add(
        "knowledge/r.md",
        "---\nid: r\ntype: record\nreferences: [system:ghost]\n---\n# R\n",
    );

    // Committed check (reads the existing committed index) is still clean.
    let (committed, _) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(committed.violations.is_empty());

    // Working-tree check reindexes from disk and catches the dangling ref.
    let (wt, failed) = commands::check::run(&c.ctx(), false, true).unwrap();
    assert!(failed);
    assert!(
        wt.violations
            .iter()
            .any(|v| matches!(v, Violation::DanglingRef { to, .. } if to == "system:ghost"))
    );
}
