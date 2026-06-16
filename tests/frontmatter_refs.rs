//! Frontmatter references: the bare `?type: descriptor` unresolved form, forgiving
//! bracket handling, and the `vaire check` guard for the `[[ ]]` trap (cli.md §6.3).

mod common;

use common::Corpus;
use vaire::commands;
use vaire::index::check::{Violation, Warning};

#[test]
fn colon_in_display_field_is_not_a_reference() {
    // A name/title with a colon must not be parsed as a `type:id` (the reported trap).
    let c = Corpus::empty();
    c.add(
        "knowledge/atlas.md",
        "---\nid: atlas\ntype: project\nname: \"IN Morning 2026 — Workshop D: Autonomous Agents\"\n---\n# Atlas\n",
    )
    .commit()
    .build();
    let (report, _) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(
        !report
            .violations
            .iter()
            .any(|v| matches!(v, Violation::DanglingRef { .. })),
        "a colon in name: must not create a dangling reference: {:?}",
        report.violations
    );
}

#[test]
fn frontmatter_reference_with_unconfigured_type_is_ignored() {
    // `team` is not in the default type vocabulary → not treated as a reference.
    let c = Corpus::empty();
    c.add(
        "knowledge/r.md",
        "---\nid: r\ntype: record\nlead: team:alpha\n---\n# R\n",
    )
    .commit()
    .build();
    let (report, _) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(
        !report
            .violations
            .iter()
            .any(|v| matches!(v, Violation::DanglingRef { to, .. } if to == "team:alpha"))
    );
    let refs = commands::refs::run(&c.ctx(), "record:r", 1, None).unwrap();
    assert!(refs.refs.iter().all(|r| r.id != "team:alpha"));

    // …but the drop is surfaced, not silent.
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::UnknownType { value, .. } if value == "team:alpha"))
    );
}

#[test]
fn check_does_not_flag_colon_in_non_reference_value() {
    // A colon-y prose value (whitespace, not a clean type:slug) must not warn.
    let c = Corpus::empty();
    c.add(
        "knowledge/r.md",
        "---\nid: r\ntype: record\nsummary: \"TODO: write this up\"\n---\n# R\n",
    )
    .commit()
    .build();
    let (report, _) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::UnknownType { .. }))
    );
}

#[test]
fn check_does_not_flag_configured_reference_type() {
    let c = Corpus::empty();
    c.add(
        "knowledge/jane.md",
        "---\nid: jane\ntype: person\nname: Jane\n---\n# Jane\n",
    )
    .add(
        "knowledge/r.md",
        "---\nid: r\ntype: record\nlead: person:jane\n---\n# R\n",
    )
    .commit()
    .build();
    let (report, _) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::UnknownType { .. }))
    );
}

#[test]
fn frontmatter_reference_with_configured_type_links() {
    let c = Corpus::empty();
    c.add(
        "knowledge/jane.md",
        "---\nid: jane\ntype: person\nname: Jane\n---\n# Jane\n",
    )
    .add(
        "knowledge/r.md",
        "---\nid: r\ntype: record\nlead: person:jane\n---\n# R\n",
    )
    .commit()
    .build();
    let bl = commands::backlinks::run(&c.ctx(), "person:jane", None, None).unwrap();
    assert!(
        bl.backlinks
            .iter()
            .any(|b| b.id == "record:r" && b.ref_type == "lead")
    );
}

#[test]
fn frontmatter_unresolved_reference_appears_in_unresolved() {
    let c = Corpus::empty();
    c.add(
        "knowledge/hr.md",
        "---\nid: hr\ntype: department\nname: HR\nhead: \"?person: someone senior\"\n---\n# HR\n",
    )
    .commit()
    .build();

    let out = commands::unresolved::run(&c.ctx(), None, None).unwrap();
    assert!(out.unresolved.iter().any(|u| {
        u.descriptor == "someone senior" && u.type_guess.as_deref() == Some("person")
    }));
}

#[test]
fn frontmatter_typeless_unresolved_reference() {
    let c = Corpus::empty();
    c.add(
        "knowledge/hr.md",
        "---\nid: hr\ntype: department\nname: HR\nhead: \"?: the new lead\"\n---\n# HR\n",
    )
    .commit()
    .build();

    let out = commands::unresolved::run(&c.ctx(), None, None).unwrap();
    let item = out
        .unresolved
        .iter()
        .find(|u| u.descriptor == "the new lead")
        .expect("typeless loose end");
    assert!(item.type_guess.is_none());
    // The work-list entry points at the field's record/line.
    assert_eq!(item.record, "department:hr");
}

#[test]
fn frontmatter_resolved_ref_with_stray_brackets_still_links() {
    let c = Corpus::empty();
    c.add(
        "knowledge/jane.md",
        "---\nid: jane\ntype: person\nname: Jane\n---\n# Jane\n",
    )
    .add(
        "knowledge/hr.md",
        // muscle-memory brackets, quoted — forgivingly stripped into a real edge
        "---\nid: hr\ntype: department\nname: HR\nhead: \"[[person:jane]]\"\n---\n# HR\n",
    )
    .commit()
    .build();

    let out = commands::backlinks::run(&c.ctx(), "person:jane", None, None).unwrap();
    assert!(
        out.backlinks
            .iter()
            .any(|b| b.id == "department:hr" && b.ref_type == "head")
    );
}

#[test]
fn check_warns_on_quoted_bracket_frontmatter() {
    let c = Corpus::empty();
    c.add(
        "knowledge/jane.md",
        "---\nid: jane\ntype: person\nname: Jane\n---\n# Jane\n",
    )
    .add(
        "knowledge/hr.md",
        "---\nid: hr\ntype: department\nname: HR\nhead: \"[[person:jane]]\"\n---\n# HR\n",
    )
    .commit()
    .build();

    let (report, _) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::FrontmatterWikilink { field, .. } if field == "head"))
    );
}

#[test]
fn check_warns_on_unquoted_bracket_frontmatter() {
    let c = Corpus::empty();
    // Unquoted `[[...]]` parses to a nested array (silent no-op) — the guard catches it.
    c.add(
        "knowledge/hr.md",
        "---\nid: hr\ntype: department\nname: HR\nhead: [[?person: Foo]]\n---\n# HR\n",
    )
    .commit()
    .build();

    let (report, _) = commands::check::run(&c.ctx(), false, false).unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::FrontmatterWikilink { field, .. } if field == "head"))
    );
}
