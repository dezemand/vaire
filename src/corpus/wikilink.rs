//! Inline wikilink scanner (design.md §6).
//!
//! Walks prose, finds every `[[...]]`, and parses each via
//! [`Reference::parse_inner`], preserving the 1-based source line so backlinks/refs
//! can report position (cli.md §3.2). Fenced code blocks (``` / ~~~) are skipped so a
//! `[[...]]` shown in an example does not become a phantom edge. The `?`-vs-not
//! distinction (edge vs loose end) is the parser's job, not this scanner's.

use crate::model::reference::Reference;

/// Scan `prose` for `[[...]]` references, returning each parsed [`Reference`] with its
/// absolute 1-based line (`prose_start_line` is the line the prose body begins on).
pub fn scan(prose: &str, prose_start_line: u32) -> Vec<(Reference, u32)> {
    let mut out = Vec::new();
    let mut in_fence = false;

    for (i, line) in prose.lines().enumerate() {
        let file_line = prose_start_line + i as u32;
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }

        let mut rest = line;
        while let Some(start) = rest.find("[[") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("]]") else { break };
            if let Some(reference) = Reference::parse_inner(&after[..end]) {
                out.push((reference, file_line));
            }
            rest = &after[end + 2..];
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
}
