//! Spec tests for `vaire render <id>` — portable Markdown with resolved links.

mod common;

use common::Corpus;
use vaire::commands;
use vaire::error::ExitCode;

#[test]
fn render_keeps_frontmatter_and_resolves_links() {
    let c = Corpus::fixture();
    let out = commands::render::run(&c.ctx(), "record:2026-06-10-broker-sync").unwrap();
    let md = out.markdown;

    // Frontmatter is kept verbatim.
    assert!(md.contains("id: 2026-06-10-broker-sync"));
    assert!(md.contains("type: record"));

    // [[person:jane-doe]] → [Jane Doe](<relative path to the file>), display from name:.
    assert!(
        md.contains("[Jane Doe]("),
        "expected resolved link, got:\n{md}"
    );
    assert!(md.contains("jane-doe.md)"));
    // No raw resolved wikilink survives.
    assert!(!md.contains("[[person:jane-doe]]"));

    // Unresolved [[?...]] render as their plain descriptor (not a link, brackets gone).
    assert!(md.contains("someone from logistics"));
    assert!(md.contains("the broker thing"));
    assert!(!md.contains("[[?"));
}

#[test]
fn render_uses_relative_paths_and_keeps_unknown_targets_verbatim() {
    let c = Corpus::empty();
    c.add("knowledge/a.md", "---\nid: a\ntype: method\nname: Method A\n---\n# A\n")
        .add(
            "knowledge/b.md",
            // resolved sibling link + a dangling one + a fenced example that must NOT change
            "---\nid: b\ntype: method\nname: B\n---\n# B\n\nSee [[method:a]] and [[method:ghost]].\n\n```\n[[method:a]]\n```\n",
        )
        .commit()
        .build();

    let md = commands::render::run(&c.ctx(), "method:b")
        .unwrap()
        .markdown;

    // Sibling file → same-directory relative href, display from name:.
    assert!(md.contains("[Method A](./a.md)"), "got:\n{md}");
    // Dangling target can't resolve → left verbatim.
    assert!(md.contains("[[method:ghost]]"));
    // The fenced code block's wikilink is untouched.
    assert!(md.contains("```\n[[method:a]]\n```") || md.contains("[[method:a]]"));
}

#[test]
fn render_unknown_id_is_exit_5() {
    let c = Corpus::fixture();
    let err = commands::render::run(&c.ctx(), "person:nobody").unwrap_err();
    assert_eq!(err.exit_code(), ExitCode::IdNotFound);
}
