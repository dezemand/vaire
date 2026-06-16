//! Display-name fallback: `name:` → sole `# H1` → filename without extension (design.md §6).

mod common;

use common::Corpus;
use vaire::commands;

/// The resolved name `vaire resolve` reports for a node.
fn name_of(c: &Corpus, id: &str) -> String {
    commands::resolve::run(&c.ctx(), id).unwrap().frontmatter["name"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn explicit_name_wins() {
    let c = Corpus::empty();
    c.add(
        "knowledge/a.md",
        "---\nid: a\ntype: method\nname: Explicit Name\n---\n# A Heading\n",
    )
    .commit()
    .build();
    assert_eq!(name_of(&c, "method:a"), "Explicit Name");
}

#[test]
fn falls_back_to_sole_h1() {
    let c = Corpus::empty();
    c.add(
        "knowledge/a.md",
        "---\nid: a\ntype: method\n---\n# The One Title\n\nbody\n",
    )
    .commit()
    .build();
    assert_eq!(name_of(&c, "method:a"), "The One Title");
}

#[test]
fn falls_back_to_filename_when_no_h1() {
    let c = Corpus::empty();
    c.add(
        "knowledge/widget-spec.md",
        "---\nid: a\ntype: method\n---\njust prose, no heading\n",
    )
    .commit()
    .build();
    // Filename without extension — note this differs from the id slug (`a`).
    assert_eq!(name_of(&c, "method:a"), "widget-spec");
}

#[test]
fn ambiguous_h1_falls_back_to_filename() {
    let c = Corpus::empty();
    c.add(
        "knowledge/notes.md",
        "---\nid: a\ntype: method\n---\n# First\n\nx\n\n# Second\n\ny\n",
    )
    .commit()
    .build();
    assert_eq!(name_of(&c, "method:a"), "notes");
}

#[test]
fn render_uses_fallback_name_for_link_text() {
    let c = Corpus::empty();
    c.add(
        "knowledge/a.md",
        "---\nid: a\ntype: method\n---\n# Event Sourcing\n",
    )
    .add(
        "knowledge/b.md",
        "---\nid: b\ntype: method\nname: B\n---\n# B\n\nSee [[method:a]].\n",
    )
    .commit()
    .build();
    let md = commands::render::run(&c.ctx(), "method:b")
        .unwrap()
        .markdown;
    // a.md has no name: but a single H1 → link text is that title.
    assert!(md.contains("[Event Sourcing](./a.md)"), "got:\n{md}");
}
