//! Shared Markdown code-awareness: fenced blocks and inline code spans.
//!
//! Several passes over prose must agree on one question — *is this text code?* Wikilink
//! scanning, `#`-title detection, section splitting, and reference rendering each used to
//! answer it with their own copy of a single `in_fence` boolean toggled by any line
//! starting with ``` or ~~~. That copy was wrong in the same way everywhere: it forgot
//! *which* marker opened the block, so a backtick fence nested inside a tilde fence (the
//! canonical way to show a fenced example) flipped the state early — turning documentation
//! into phantom graph edges and swallowing the real links that followed. Section splitting
//! had no fence tracking at all, so a `## ` comment inside a shell example became a
//! section heading and a search anchor pointing into a code block.
//!
//! This module is the single answer, following CommonMark: a fence closes only on the same
//! marker character, at least as long as the one that opened it, with nothing after it.

/// Tracks whether the line-by-line walk is currently inside a fenced code block.
///
/// Feed every line in order to [`Fences::is_code`]; the state machine is per-document.
#[derive(Debug, Default)]
pub struct Fences {
    /// The marker that opened the current block: its character and its run length.
    open: Option<(char, usize)>,
}

impl Fences {
    pub fn new() -> Self {
        Fences::default()
    }

    /// Feed the next line. Returns `true` when the line is code — either a fence delimiter
    /// itself or a line inside a fenced block — and `false` when it is ordinary prose.
    ///
    /// CommonMark: an opening fence may be indented up to three spaces and may carry an
    /// info string; a *closing* fence must use the same character, run at least as long,
    /// and be followed by nothing but whitespace. Tracking the opening marker is what makes
    /// ` ``` ` inside a `~~~` block stay code instead of ending the block.
    pub fn is_code(&mut self, line: &str) -> bool {
        let trimmed = line.trim_start();
        match (self.open, fence_marker(trimmed)) {
            // Not in a block: a fence here opens one.
            (None, Some(marker)) => {
                self.open = Some(marker);
                true
            }
            (None, None) => false,
            // Inside a block: only a matching bare fence closes it.
            (Some((open_char, open_len)), Some((char_, len))) => {
                if char_ == open_char && len >= open_len && is_bare_fence(trimmed, len) {
                    self.open = None;
                }
                true
            }
            (Some(_), None) => true,
        }
    }

    /// Whether the walk is currently inside a fenced block.
    pub fn in_fence(&self) -> bool {
        self.open.is_some()
    }
}

/// The fence marker opening `trimmed`, as (character, run length), if it is one.
fn fence_marker(trimmed: &str) -> Option<(char, usize)> {
    let char_ = trimmed.chars().next()?;
    if char_ != '`' && char_ != '~' {
        return None;
    }
    let len = trimmed.chars().take_while(|&c| c == char_).count();
    (len >= 3).then_some((char_, len))
}

/// Whether the fence carries no info string — the CommonMark requirement for a *closing*
/// fence. (`~~~rust` opens a block; a bare `~~~` closes it.)
fn is_bare_fence(trimmed: &str, marker_len: usize) -> bool {
    trimmed.chars().skip(marker_len).all(char::is_whitespace)
}

/// Blank out inline code spans, replacing their characters (delimiters included) with
/// spaces so the line keeps its shape.
///
/// In CommonMark, code spans bind tighter than links: `` `[text](url)` `` renders as
/// literal text, not a link. Wikilinks follow the same precedence, so writing
/// ``write `[[person:jane]]` to link a person`` documents the syntax instead of quietly
/// creating an edge to `person:jane`.
///
/// A span opens on a run of N backticks and closes on the next run of exactly N; an
/// unterminated run is literal text and is left alone.
pub fn mask_code_spans(line: &str) -> String {
    if !line.contains('`') {
        return line.to_string();
    }
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<char> = chars.clone();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '`' {
            i += 1;
            continue;
        }
        let open_len = chars[i..].iter().take_while(|&&c| c == '`').count();
        // Find a closing run of exactly `open_len` backticks.
        let mut j = i + open_len;
        let close = loop {
            if j >= chars.len() {
                break None;
            }
            if chars[j] == '`' {
                let run = chars[j..].iter().take_while(|&&c| c == '`').count();
                if run == open_len {
                    break Some(j);
                }
                j += run;
            } else {
                j += 1;
            }
        };
        match close {
            Some(close) => {
                for slot in out.iter_mut().take(close + open_len).skip(i) {
                    *slot = ' ';
                }
                i = close + open_len;
            }
            // Unterminated: literal backticks, nothing to mask.
            None => i += open_len,
        }
    }
    out.into_iter().collect()
}

/// Extract relative link/image targets with 1-based line numbers: inline
/// `[text](target)` / `![alt](target)` plus reference-style definitions
/// (`[label]: target` on its own line). Shared by `vaire pack` (registry.md §11 —
/// inclusion rides on extraction) and diagram-file discovery (a fenced-off `.puml` link
/// is found the same way a packed asset link is). Code is skipped via the fence/code-span
/// awareness above, HTML (`<img src>`) is out of scope, and anything URL-shaped
/// (`https://…`, `mailto:`, bare `#anchor`) is ignored. Wikilinks (`[[type:id]]`) never
/// match: they have no `](`.
pub fn relative_link_targets(content: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let mut fences = Fences::new();
    for (i, raw_line) in content.lines().enumerate() {
        let line_no = (i + 1) as u32;
        if fences.is_code(raw_line) {
            continue;
        }
        // A link inside an inline code span is an example, not a demand.
        let masked = mask_code_spans(raw_line);
        let trimmed = masked.trim_start();
        // Reference-style definition: `[label]: target` alone on a line. Footnotes
        // (`[^…]`) are prose; a label containing brackets means this was actually an
        // inline link followed by a colon, which the `](` scan below handles.
        if let Some(rest) = trimmed.strip_prefix('[')
            && !rest.starts_with('^')
            && let Some((label, def)) = rest.split_once("]:")
            && !label.contains(['[', ']'])
        {
            let def = def.trim();
            let raw_target = match def.strip_prefix('<') {
                Some(bracketed) => bracketed
                    .split_once('>')
                    .map(|(t, _)| t)
                    .unwrap_or(bracketed),
                None => def.split(char::is_whitespace).next().unwrap_or(def),
            };
            if let Some(target) = classify(raw_target) {
                out.push((target, line_no));
            }
            continue;
        }
        let mut rest = masked.as_str();
        while let Some(idx) = rest.find("](") {
            let after = &rest[idx + 2..];
            let (raw_target, remainder) = match after.strip_prefix('<') {
                // `](<path with spaces>)` — the brackets exist to permit spaces, so no
                // title-splitting applies inside them.
                Some(bracketed) => match bracketed.split_once('>') {
                    Some((t, r)) => (t, r),
                    None => break,
                },
                None => match after.split_once(')') {
                    // `](path "title")` — the title is not part of the path.
                    Some((t, r)) => (t.split(char::is_whitespace).next().unwrap_or(t), r),
                    None => break,
                },
            };
            rest = remainder;
            if let Some(target) = classify(raw_target) {
                out.push((target, line_no));
            }
        }
    }
    out
}

/// Reduce a raw link target to a relative file path worth checking, or `None` for
/// targets that are not package files (URLs, anchors, empty). A leading-`/` "absolute"
/// path is returned as-is so resolution can flag it — corpus Markdown is portable and an
/// absolute path is broken everywhere but one machine.
fn classify(raw: &str) -> Option<String> {
    let mut target = raw.trim();
    // `path#fragment` — the file is what must exist.
    if let Some((path, _fragment)) = target.split_once('#') {
        target = path;
    }
    if target.is_empty() {
        return None;
    }
    // A scheme (`https://…`, `mailto:…`, `tel:…`) marks an external target: a colon
    // before any path separator. Relative file paths cannot contain one there.
    let head = target.split('/').next().unwrap_or(target);
    if head.contains(':') {
        return None;
    }
    Some(target.to_string())
}

/// Resolve `target` relative to `source` (both `/`-separated, package-root-relative).
/// `None` when the target escapes the package root (including absolute paths).
pub fn resolve_relative(source: &str, target: &str) -> Option<String> {
    if target.starts_with('/') {
        return None;
    }
    let mut stack: Vec<&str> = source.split('/').collect();
    stack.pop(); // the source file itself; links resolve from its directory
    for comp in target.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            other => stack.push(other),
        }
    }
    Some(stack.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_lines(prose: &str) -> Vec<bool> {
        let mut fences = Fences::new();
        prose.lines().map(|l| fences.is_code(l)).collect()
    }

    #[test]
    fn backtick_fence_inside_tilde_fence_stays_code() {
        // The canonical way to show a fenced example. The old single-bool toggle flipped
        // on the inner ``` and treated the example's contents as live prose.
        let prose = "~~~\n```\ninner\n```\n~~~\nafter";
        assert_eq!(
            code_lines(prose),
            [true, true, true, true, true, false],
            "only the trailing line is prose"
        );
    }

    #[test]
    fn odd_nesting_does_not_invert_the_state() {
        // A single inner ``` used to leave in_fence stuck on, suppressing every real
        // reference in the rest of the document.
        let prose = "~~~\n```\n~~~\nafter";
        assert_eq!(code_lines(prose), [true, true, true, false]);
    }

    #[test]
    fn longer_fence_is_needed_to_close_a_longer_opener() {
        let prose = "````\n```\nstill code\n````\nafter";
        assert_eq!(code_lines(prose), [true, true, true, true, false]);
    }

    #[test]
    fn info_string_opens_but_does_not_close() {
        let prose = "```rust\nlet x = 1;\n```\nafter";
        assert_eq!(code_lines(prose), [true, true, true, false]);
    }

    #[test]
    fn masks_inline_code_spans_preserving_shape() {
        let line = "write `[[person:jane]]` here";
        let masked = mask_code_spans(line);
        assert!(
            !masked.contains("[["),
            "the wikilink must not survive masking"
        );
        assert!(masked.starts_with("write "));
        assert!(masked.ends_with(" here"));
        assert_eq!(masked.chars().count(), line.chars().count());
    }

    #[test]
    fn double_backtick_span_masks_embedded_single_backticks() {
        let line = "``a ` b`` tail";
        let masked = mask_code_spans(line);
        assert!(!masked.contains('`'), "the whole span is masked");
        assert!(masked.ends_with(" tail"));
        assert_eq!(masked.chars().count(), line.chars().count());
    }

    #[test]
    fn unterminated_backtick_is_literal_text() {
        assert_eq!(
            mask_code_spans("a ` [[person:jane]]"),
            "a ` [[person:jane]]"
        );
    }

    #[test]
    fn extracts_relative_targets_with_lines() {
        let md = "# T\n\nSee [spec](docs/spec.md) and ![wiring](../attachments/w.png).\n\n\
                  ```\n[not a link](inside/fence.md)\n```\n\nAlso [ext](https://x.example) \
                  and [anchor](#top) and [mail](mailto:a@b.c).\n";
        let links = relative_link_targets(md);
        assert_eq!(
            links,
            vec![
                ("docs/spec.md".to_string(), 3),
                ("../attachments/w.png".to_string(), 3),
            ]
        );
    }

    #[test]
    fn titles_are_split_in_extraction() {
        let md = "[doc](a.md \"the title\")\n";
        assert_eq!(relative_link_targets(md), vec![("a.md".to_string(), 1)]);
    }

    #[test]
    fn code_awareness_is_shared_with_the_corpus() {
        let md = "~~~\n```\n[not](a.md)\n```\n[not](b.md)\n~~~\n\
                  [real](c.md) and `[not](d.md)`\n";
        assert_eq!(relative_link_targets(md), vec![("c.md".to_string(), 7)]);
    }

    #[test]
    fn reference_style_definitions_extract() {
        let md = "![photo][p]\n\n[p]: assets/photo.jpg\n[q]: <my file.png>\n\
                  [^fn]: a footnote, not a file\n[r]: https://x.example\n\
                  [text](a.md): an inline link before a colon is not a definition\n";
        assert_eq!(
            relative_link_targets(md),
            vec![
                ("assets/photo.jpg".to_string(), 3),
                ("my file.png".to_string(), 4),
                ("a.md".to_string(), 7),
            ]
        );
    }

    #[test]
    fn classify_strips_fragments_and_schemes() {
        assert_eq!(classify("a.md#sec"), Some("a.md".into()));
        assert_eq!(classify("#only-anchor"), None);
        assert_eq!(classify("https://x.example/p"), None);
        assert_eq!(classify("tel:123"), None);
        assert_eq!(classify(""), None);
        assert_eq!(classify("/etc/passwd"), Some("/etc/passwd".into()));
    }

    #[test]
    fn angle_bracket_targets_keep_spaces() {
        let md = "[doc](<my file.md>)\n";
        assert_eq!(
            relative_link_targets(md),
            vec![("my file.md".to_string(), 1)]
        );
    }

    #[test]
    fn resolution_is_directory_relative_and_containment_checked() {
        assert_eq!(
            resolve_relative("knowledge/a/b.md", "../x.md"),
            Some("knowledge/x.md".into())
        );
        assert_eq!(
            resolve_relative("readme.md", "attachments/p.png"),
            Some("attachments/p.png".into())
        );
        assert_eq!(resolve_relative("a.md", "../../out.md"), None);
        assert_eq!(resolve_relative("a.md", "/abs.md"), None);
    }
}
