//! Diagram references become graph edges (design.md §6, issue #23).
//!
//! vaire does not parse PlantUML, Mermaid, or draw.io. It scans the **raw text** of a
//! diagram source for link targets that begin with the literal prefix `vaire/`, strips
//! the prefix, and parses what follows with the ordinary reference grammar (`type:id`,
//! `@pkg/type:id`, scoped `container-id/type:local` — the same grammar `[[...]]`
//! wikilinks use, `model::reference`). The marker is ours, not corpus grammar: the
//! prefix survives every diagram tool because it is not a URI scheme (some tools drop an
//! href that reads as one under their default security level), and it makes the scan
//! unambiguous.
//!
//! Two places feed this scanner:
//! - a fenced ```` ```plantuml ````/```` ```mermaid ```` block in a node's own prose
//!   (pure — [`scan_prose`], called from `corpus::frontmatter::to_node`);
//! - an external diagram file a node links to (impure — the file has to be read; the
//!   caller in `index::build` fetches its content and calls [`scan_source`]).
//!
//! Both funnel through [`scan_source`], which finds every `vaire/`-prefixed target in a
//! block of text and reports each as either a parsed [`NodeId`] or a malformed target —
//! a typo must not evaporate.

use crate::model::id::NodeId;

use super::markdown::Fences;

/// Fenced-block info strings recognised as diagram sources.
pub const DIAGRAM_FENCE_LANGS: &[&str] = &["plantuml", "puml", "uml", "mermaid"];

/// File extensions recognised as diagram sources (`.drawio.svg`/`.drawio.png` are
/// already pictures, not sources, and are deliberately absent).
pub const DIAGRAM_EXTENSIONS: &[&str] = &[
    ".puml",
    ".plantuml",
    ".pu",
    ".iuml",
    ".mmd",
    ".mermaid",
    ".drawio",
    ".dio",
];

/// Whether `path` names a diagram source file by extension.
pub fn is_diagram_path(path: &str) -> bool {
    DIAGRAM_EXTENSIONS.iter().any(|ext| path.ends_with(ext))
}

/// The literal prefix a diagram link target must start with to be a vairë reference.
const PREFIX: &str = "vaire/";

/// Characters an address (the part after `vaire/`) may contain; the address ends at the
/// first character outside this set.
fn is_address_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '@' | '-')
}

/// One `vaire/`-prefixed target found in a diagram source, with its 1-based line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagramMarker {
    /// Parsed as a well-formed reference address.
    Resolved(NodeId),
    /// The `vaire/`-prefixed target did not parse as `type:id` / `@pkg/type:id` /
    /// `container-id/type:local` — reported, not silently dropped, so a typo doesn't
    /// evaporate.
    Malformed(String),
}

/// Scan raw diagram source text for `vaire/`-prefixed link targets.
///
/// Scan rules (design.md §6): the marker must **start** a target — if the character
/// immediately before `vaire/` is itself an address character, it's skipped, so
/// `https://example.com/vaire/x` (somebody's URL) is not matched. Trailing `.` and `/`
/// are trimmed (sentence punctuation in a diagram note). Results are de-duplicated,
/// first-occurrence order.
pub fn scan_source(content: &str) -> Vec<(DiagramMarker, u32)> {
    let mut out: Vec<(DiagramMarker, u32)> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for (i, line) in content.lines().enumerate() {
        let line_no = (i + 1) as u32;
        let mut idx = 0usize;
        while let Some(found) = line[idx..].find(PREFIX) {
            let start = idx + found;
            let preceded_by_address_char = start > 0
                && line[..start]
                    .chars()
                    .next_back()
                    .is_some_and(is_address_char);
            let after = &line[start + PREFIX.len()..];
            let addr_len = after
                .char_indices()
                .find(|(_, c)| !is_address_char(*c))
                .map(|(i, _)| i)
                .unwrap_or(after.len());
            let raw = after[..addr_len].trim_end_matches(['.', '/']);

            if preceded_by_address_char || raw.is_empty() {
                idx = start + PREFIX.len();
                continue;
            }

            let marker = match raw.parse::<NodeId>() {
                Ok(id) => DiagramMarker::Resolved(id),
                Err(_) => DiagramMarker::Malformed(raw.to_string()),
            };
            let key = match &marker {
                DiagramMarker::Resolved(id) => format!("r:{id}"),
                DiagramMarker::Malformed(raw) => format!("m:{raw}"),
            };
            if seen.insert(key) {
                out.push((marker, line_no));
            }

            idx = start + PREFIX.len() + addr_len;
        }
    }
    out
}

/// Scan a node's own prose for fenced diagram blocks (```` ```plantuml ````, ```` ```mermaid
/// ````, etc.) and return every `vaire/`-prefixed marker found inside them, with the
/// **absolute** file line (`prose_start_line` is the line the prose body begins on) —
/// per the issue, the source of a fenced-block marker is the node's own file.
pub fn scan_prose(prose: &str, prose_start_line: u32) -> Vec<(DiagramMarker, u32)> {
    let mut out = Vec::new();
    let mut fences = Fences::new();
    let mut in_diagram_fence = false;
    let mut block = String::new();
    let mut block_start_line = 0u32;

    for (i, line) in prose.lines().enumerate() {
        let file_line = prose_start_line + i as u32;
        let was_in_fence = fences.in_fence();
        let is_code = fences.is_code(line);

        if !was_in_fence && is_code {
            // Just opened a fence: check its info string.
            let trimmed = line.trim_start();
            let lang = trimmed.trim_start_matches(['`', '~']).trim();
            if DIAGRAM_FENCE_LANGS.contains(&lang) {
                in_diagram_fence = true;
                block.clear();
                block_start_line = file_line + 1;
            }
            continue;
        }
        if was_in_fence && !fences.in_fence() {
            // Just closed a fence.
            if in_diagram_fence {
                for (marker, rel_line) in scan_source(&block) {
                    out.push((marker, block_start_line + rel_line - 1));
                }
                in_diagram_fence = false;
            }
            continue;
        }
        if in_diagram_fence && is_code {
            block.push_str(line);
            block.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plantuml_marker_resolves() {
        let content = r#"component "API Gateway" [[vaire/system:gateway]] #FFD966"#;
        let refs = scan_source(content);
        assert_eq!(refs.len(), 1);
        match &refs[0].0 {
            DiagramMarker::Resolved(id) => assert_eq!(id.to_string(), "system:gateway"),
            other => panic!("expected resolved, got {other:?}"),
        }
    }

    #[test]
    fn mermaid_click_marker_resolves_scoped_and_cross_package() {
        let content = "click A href \"vaire/@acme/person:jane-doe\"";
        let refs = scan_source(content);
        assert_eq!(refs.len(), 1);
        match &refs[0].0 {
            DiagramMarker::Resolved(id) => assert_eq!(id.to_string(), "@acme/person:jane-doe"),
            other => panic!("expected resolved, got {other:?}"),
        }
    }

    #[test]
    fn url_containing_prefix_is_not_matched() {
        let refs = scan_source("note: see https://example.com/vaire/x for details");
        assert!(refs.is_empty(), "got {refs:?}");
    }

    #[test]
    fn trailing_punctuation_is_trimmed() {
        let refs = scan_source("See vaire/system:gateway.");
        match &refs[0].0 {
            DiagramMarker::Resolved(id) => assert_eq!(id.to_string(), "system:gateway"),
            other => panic!("expected resolved, got {other:?}"),
        }
    }

    #[test]
    fn duplicates_collapse_to_first_occurrence() {
        let refs = scan_source("vaire/system:gateway\nvaire/system:gateway\n");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].1, 1);
    }

    #[test]
    fn malformed_target_is_reported_not_dropped() {
        // Address-charset text with no `:` doesn't parse as `type:id` — a typo, not a
        // URL, so it must be reported rather than silently skipped.
        let refs = scan_source("vaire/not-a-reference");
        assert_eq!(refs.len(), 1);
        match &refs[0].0 {
            DiagramMarker::Malformed(raw) => assert_eq!(raw, "not-a-reference"),
            other => panic!("expected malformed, got {other:?}"),
        }
    }

    #[test]
    fn scan_prose_finds_fenced_plantuml_block_with_absolute_line() {
        let prose = "# Title\n\n```plantuml\ncomponent A [[vaire/system:gateway]]\n```\n";
        let refs = scan_prose(prose, 10);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].1, 13); // fence opens at abs line 12, body at 13
    }

    #[test]
    fn scan_prose_ignores_non_diagram_fences() {
        let prose = "```rust\nlet x = \"vaire/system:gateway\";\n```\n";
        let refs = scan_prose(prose, 1);
        assert!(refs.is_empty(), "got {refs:?}");
    }

    #[test]
    fn is_diagram_path_recognises_every_extension() {
        for ext in DIAGRAM_EXTENSIONS {
            assert!(is_diagram_path(&format!("diagrams/x{ext}")), "{ext}");
        }
        assert!(!is_diagram_path("diagrams/x.drawio.svg"));
        assert!(!is_diagram_path("diagrams/x.drawio.png"));
        assert!(!is_diagram_path("readme.md"));
    }
}
