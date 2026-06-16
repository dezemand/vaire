//! Section splitting for embeddings (design.md §9).
//!
//! Sections split on `##` headings; each chunk is embedded independently, but the
//! **file is the returned unit** for search. A section also yields the heading + line
//! that become a search result's `anchor` (cli.md §3.4). Text before the first `##`
//! (including a `#` title) is the preamble section, with `heading: None`.

/// One embeddable chunk of a node's prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The heading text (without the `##`), or `None` for the pre-heading preamble.
    pub heading: Option<String>,
    /// 1-based line of the heading (or body start for the preamble) — the search anchor.
    pub line: u32,
    /// The section body, the text actually embedded and FTS-indexed.
    pub body: String,
}

impl Section {
    /// Split `prose` into sections on `##` headings. `prose_start_line` makes line
    /// numbers absolute within the file. Only level-2 headings split; deeper headings
    /// fold into their enclosing section.
    pub fn split(prose: &str, prose_start_line: u32) -> Vec<Section> {
        let mut sections = Vec::new();
        let mut heading: Option<String> = None;
        let mut line = prose_start_line;
        let mut body = String::new();
        let mut open = false;

        let mut flush = |heading: &mut Option<String>, line: u32, body: &mut String| {
            sections.push(Section {
                heading: heading.take(),
                line,
                body: body.trim().to_string(),
            });
            body.clear();
        };

        for (i, raw) in prose.lines().enumerate() {
            let file_line = prose_start_line + i as u32;
            if let Some(h) = raw.strip_prefix("## ") {
                if open {
                    flush(&mut heading, line, &mut body);
                }
                heading = Some(h.trim().to_string());
                line = file_line;
                open = true;
            } else {
                if !open {
                    open = true;
                    heading = None;
                    line = file_line;
                }
                body.push_str(raw);
                body.push('\n');
            }
        }
        if open {
            flush(&mut heading, line, &mut body);
        }
        sections
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_preamble_and_level_two_headings() {
        let prose = "# Title\n\nintro text\n\n## Background\n\nbody one\n\n## Decision\n\nbody two";
        let sections = Section::split(prose, 9);

        assert_eq!(sections.len(), 3);

        assert_eq!(sections[0].heading, None);
        assert_eq!(sections[0].line, 9);
        assert!(sections[0].body.contains("intro text"));

        assert_eq!(sections[1].heading.as_deref(), Some("Background"));
        assert_eq!(sections[1].line, 13);
        assert_eq!(sections[1].body, "body one");

        assert_eq!(sections[2].heading.as_deref(), Some("Decision"));
        assert_eq!(sections[2].body, "body two");
    }
}
