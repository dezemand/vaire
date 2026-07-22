//! Inline wikilink scanner (design.md §6).
//!
//! Walks prose, finds every `[[...]]`, and parses each via
//! [`Reference::parse_inner`], preserving the 1-based source line so backlinks/refs
//! can report position (cli.md §3.2). Code is skipped — both fenced blocks (``` / ~~~)
//! and inline `` `code spans` `` — so a `[[...]]` shown in an example does not become a
//! phantom edge; see [`crate::corpus::markdown`]. The `?`-vs-not distinction (edge vs
//! loose end) is the parser's job, not this scanner's.

use crate::corpus::markdown::{Fences, mask_code_spans};
use crate::model::reference::Reference;

/// Scan `prose` for `[[...]]` references, returning each parsed [`Reference`] with its
/// absolute 1-based line (`prose_start_line` is the line the prose body begins on).
pub fn scan(prose: &str, prose_start_line: u32) -> Vec<(Reference, u32)> {
    let mut out = Vec::new();
    let mut fences = Fences::new();

    for (i, line) in prose.lines().enumerate() {
        let file_line = prose_start_line + i as u32;
        if fences.is_code(line) {
            continue;
        }
        let line = mask_code_spans(line);

        let mut rest = line.as_str();
        while let Some(start) = rest.find("[[") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("]]") else { break };
            if let Some(reference) = Reference::parse_inner(&after[..end]) {
                out.push((reference, file_line));
                rest = &after[end + 2..];
            } else {
                // Resync from just past this `[[` rather than past the `]]`: the span we
                // just rejected may itself contain a genuine link's opener, as in
                // `use [[ to open a link, e.g. [[person:jane]]`. Skipping to the `]]`
                // would consume that link and silently drop the edge.
                rest = after;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::reference::Reference;

    #[test]
    fn scans_resolved_and_unresolved_with_lines() {
        let prose = "# Title\n\n[[person:jane-doe]] met [[dept:logistics|the logistics team]].\n[[?person: someone from logistics]] spoke.";
        let refs = scan(prose, 9);

        assert_eq!(refs.len(), 3);
        // Two refs on the first content line (line 11), one on the next (line 12).
        assert_eq!(refs[0].1, 11);
        assert_eq!(refs[1].1, 11);
        assert_eq!(refs[2].1, 12);

        assert!(matches!(refs[0].0, Reference::Resolved { .. }));
        assert!(matches!(refs[2].0, Reference::Unresolved { .. }));
    }

    #[test]
    fn skips_fenced_code() {
        let prose = "```\n[[person:not-an-edge]]\n```\n[[person:real]]";
        let refs = scan(prose, 1);
        assert_eq!(refs.len(), 1);
        match &refs[0].0 {
            Reference::Resolved { target, .. } => assert_eq!(target.slug, "real"),
            _ => panic!("expected resolved"),
        }
    }

    #[test]
    fn backtick_fence_nested_in_tilde_fence_makes_no_phantom_edge() {
        // Showing a fenced example inside a tilde fence is the canonical use of ~~~.
        // The old single-bool toggle flipped on the inner ``` and scanned the example.
        let prose = "~~~\n```\n[[person:phantom]]\n```\n~~~\n[[person:real]]";
        let refs = scan(prose, 1);
        assert_eq!(refs.len(), 1, "only the link outside the block is an edge");
        match &refs[0].0 {
            Reference::Resolved { target, .. } => assert_eq!(target.slug, "real"),
            _ => panic!("expected resolved"),
        }
    }

    #[test]
    fn odd_fence_nesting_does_not_suppress_later_links() {
        // Unbalanced inner fence used to leave the scanner stuck "inside" a block,
        // silently dropping every remaining reference in the file.
        let prose = "~~~\n```\n~~~\n[[person:real]]";
        let refs = scan(prose, 1);
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn inline_code_span_is_not_an_edge() {
        // Code spans bind tighter than links in CommonMark, so documenting the syntax
        // must not create an edge to the node used in the example.
        let refs = scan("To link Jane, write `[[person:jane-doe]]` in your note.", 1);
        assert!(refs.is_empty(), "got {refs:?}");
    }

    #[test]
    fn stray_open_bracket_does_not_swallow_the_next_link() {
        // The failed span contains the real link's opener; skipping to the `]]` consumed
        // it and dropped the edge entirely.
        let refs = scan("use [[ to open a link, e.g. [[person:jane-doe]].", 1);
        assert_eq!(refs.len(), 1, "the genuine link is still found");
        match &refs[0].0 {
            Reference::Resolved { target, .. } => assert_eq!(target.slug, "jane-doe"),
            _ => panic!("expected resolved"),
        }
    }
}
