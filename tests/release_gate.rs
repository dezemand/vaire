//! The v0.2.0 release gate: issue #2's acceptance transcript, end to end, in order.
//! Green here = the headline feature is real.

mod common;

use common::Ws;
use vaire::commands;
use vaire::index::check::Violation;

#[test]
fn the_full_acceptance_transcript() {
    let ws = Ws::acceptance();
    let ctx = ws.ctx("acme-web");

    // vaire index — this package + the linked closure, one line per dep.
    let indexed = commands::index::run(&ctx, false, false, false, false).unwrap();
    assert!(indexed.dependencies.iter().all(|d| d.status == "indexed"));

    // vaire resolve @acme-core/team:platform → the acme-core file.
    let resolved = commands::resolve::run(&ctx, "@acme-core/team:platform").unwrap();
    assert_eq!(resolved.path, "knowledge/platform.md");
    assert_eq!(resolved.package.as_deref(), Some("acme-core"));

    // vaire refs service:checkout → its @acme-core / @acme-shared edges.
    let refs = commands::refs::run(&ctx, "service:checkout", 1, None).unwrap();
    let ids: Vec<&str> = refs.refs.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"@acme-core/team:platform"), "{ids:?}");
    assert!(ids.contains(&"@acme-shared/site:hq"), "{ids:?}");

    // vaire backlinks @acme-core/team:platform → services in acme-web that reference it.
    let back = commands::backlinks::run(&ctx, "@acme-core/team:platform", None, None).unwrap();
    assert!(
        back.backlinks.iter().any(|b| b.id == "service:checkout"),
        "{:?}",
        back.backlinks.iter().map(|b| &b.id).collect::<Vec<_>>()
    );

    // vaire check → wiki:… dangling cross-package (error); team/person/site resolve
    // clean; the acme-core↔acme-web cycle terminates (completion proves it).
    let (report, failed) = commands::check::run(&ctx, false, false, false).unwrap();
    assert!(failed);
    let danglings: Vec<&str> = report
        .violations
        .iter()
        .filter_map(|v| match v {
            Violation::DanglingRef { to, .. } => Some(to.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        danglings,
        ["@acme-shared/wiki:home"],
        "exactly the planted hole"
    );
    // Cross-package frontmatter values are classified by their OWNER's vocabulary —
    // never flagged unknown_type against this package's `types` (caught live in the
    // demo workspace: owner/site/wiki all warned spuriously).
    assert!(
        !report.warnings.iter().any(|w| matches!(
            w,
            vaire::index::check::Warning::UnknownType { value, .. } if value.starts_with('@')
        )),
        "{:?}",
        report.warnings
    );

    // vaire deps → acme-core ^1, acme-shared ^1 (resolved).
    let deps = commands::deps::run(&ctx).unwrap();
    for name in ["acme-core", "acme-shared"] {
        let dep = deps.dependencies.iter().find(|d| d.name == name).unwrap();
        assert_eq!(dep.constraint, "^1");
        assert!(dep.resolved.is_some(), "{name} resolved");
    }
}

/// The committed `examples/workspace/` must actually work: copy it out, link it up with
/// the real commands, and run the transcript's core against it — shipped examples never
/// rot silently.
#[test]
fn the_shipped_example_workspace_is_usable() {
    let tmp = tempfile::tempdir().unwrap();
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/workspace");
    for pkg in ["acme-core", "acme-web", "acme-shared"] {
        copy_tree(&src.join(pkg), &tmp.path().join(pkg));
    }

    let web = tmp.path().join("acme-web");
    for dep in ["acme-core", "acme-shared"] {
        commands::add::run(Some(&web), None, dep, Some(&tmp.path().join(dep))).unwrap();
    }
    let ctx = vaire::commands::Ctx::new(Some(web), None).unwrap();
    commands::index::run(&ctx, false, false, false, false).unwrap();

    let resolved = commands::resolve::run(&ctx, "@acme-core/team:platform").unwrap();
    assert_eq!(resolved.frontmatter["name"], "Platform Team");
    let (report, failed) = commands::check::run(&ctx, false, false, false).unwrap();
    assert!(failed, "the planted wiki:home dangling ref is caught");
    assert!(
        report.violations.iter().any(
            |v| matches!(v, Violation::DanglingRef { to, .. } if to == "@acme-shared/wiki:home")
        )
    );
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).unwrap();
        }
    }
}
