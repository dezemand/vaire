//! Shared text helpers for lexical search scoring.
//!
//! Tokenization here mirrors Tantivy's `default` index tokenizer exactly: Turso indexes
//! `sections(heading, body)` with `SimpleTokenizer` (split on non-alphanumeric) +
//! `RemoveLongFilter(40)` + `LowerCaser` — no stemming, no stop-word removal, no accent
//! folding. Scoring `fts_match`'s candidates with a tokenizer that disagreed with the one
//! that built the index would silently mis-score rows the index considered a hit; keeping
//! the two in lockstep is the whole point of this module.
//!
//! Every match here is a **whole token**, never a substring (the old scorer counted "an"
//! inside "and"/"can"/"want", and "end" inside "endpoint").

use std::collections::HashMap;

/// The longest token Tantivy's `RemoveLongFilter` keeps. A longer run of alphanumerics is
/// dropped by the index itself, so it must be dropped here too, or a Rust-side term-
/// frequency count would include tokens `fts_match` never indexed.
const MAX_TOKEN_LEN: usize = 40;

/// Split `text` into lowercase alphanumeric tokens, dropping anything longer than
/// [`MAX_TOKEN_LEN`] chars. Unlike [`crate::search`]'s query-side tokenizer, this keeps
/// duplicates in order — callers count frequencies from it.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty() && t.chars().count() <= MAX_TOKEN_LEN)
        .map(str::to_lowercase)
        .collect()
}

/// Token -> occurrence-count table for one piece of text (a section's heading or body),
/// for whole-token term-frequency lookups.
pub fn term_counts(text: &str) -> HashMap<String, u32> {
    let mut counts = HashMap::new();
    for t in tokenize(text) {
        *counts.entry(t).or_insert(0u32) += 1;
    }
    counts
}

/// If `body`'s first line is a Markdown H1 (`# Title`, not a `##` section heading), split
/// it out as heading-weighted text: `(Some(title), rest_of_body)`. `None` (with `body`
/// unchanged) when there is no such line.
///
/// Only meaningful where a section's own `heading` column is empty — the preamble section
/// (`corpus::section::Section::split`: only `##` headings populate `heading`), so a file's
/// `# Title` line is otherwise indistinguishable prose sitting in the preamble's BODY.
/// Without this, a query matching only the document's title would score it as an ordinary
/// body hit.
pub fn split_title_line(body: &str) -> (Option<&str>, &str) {
    let Some(rest) = body.strip_prefix("# ") else {
        return (None, body);
    };
    match rest.find('\n') {
        Some(i) => (Some(rest[..i].trim()), rest[i + 1..].trim_start()),
        None => (Some(rest.trim()), ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_lowercases_and_drops_long_tokens() {
        let long = "a".repeat(41);
        let text = format!("Endpoint, end! AND {long} ok");
        assert_eq!(tokenize(&text), vec!["endpoint", "end", "and", "ok"]);
    }

    #[test]
    fn tokenize_keeps_a_token_at_exactly_the_length_cap() {
        let exactly_40 = "a".repeat(40);
        assert_eq!(tokenize(&exactly_40), vec![exactly_40]);
    }

    #[test]
    fn term_counts_is_whole_token_not_substring() {
        // "an" must not be inflated by "and"/"can"/"want" containing it.
        let counts = term_counts("and can want an an");
        assert_eq!(counts.get("an").copied(), Some(2));
        assert_eq!(counts.get("and").copied(), Some(1));
        assert_eq!(counts.get("can").copied(), Some(1));
        assert_eq!(counts.get("want").copied(), Some(1));
    }

    #[test]
    fn split_title_line_extracts_h1_not_h2() {
        let (title, rest) = split_title_line("# My Title\n\nSome intro text.");
        assert_eq!(title, Some("My Title"));
        assert_eq!(rest, "Some intro text.");

        let (title, rest) = split_title_line("## Not a title\n\nbody");
        assert_eq!(title, None);
        assert_eq!(rest, "## Not a title\n\nbody");

        let (title, rest) = split_title_line("no title here");
        assert_eq!(title, None);
        assert_eq!(rest, "no title here");
    }

    #[test]
    fn split_title_line_handles_title_only_body() {
        let (title, rest) = split_title_line("# Only Title");
        assert_eq!(title, Some("Only Title"));
        assert_eq!(rest, "");
    }
}
