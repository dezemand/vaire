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
        assert!(!masked.contains("[["), "the wikilink must not survive masking");
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
}
