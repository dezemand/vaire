//! Deterministic English inflection helper — no crates, no dictionary.
//!
//! Turso's FTS tokenizer does no stemming, so a query for "entities" never reaches a
//! section that only says "entity". This module expands a single token into a short list
//! of *plausible* morphological variants — plural/singular, and `-ing`/`-ed` verb forms —
//! so the caller can add them to the lexical query as extra, down-weighted terms.
//!
//! Deliberately conservative and occasionally wrong: it has no dictionary, so it cannot
//! know whether a given token is actually a base form or already inflected, and English
//! morphology has real exceptions (consonant doubling, "long vowel" `-VC` endings that
//! don't double, etc.). A handful of generated candidates are not real English words
//! (e.g. stripping "es" from "releases" also yields the nonsense stem "releas" alongside
//! the correct "release"). That's fine here: a bogus candidate essentially never
//! collides with a real corpus token, and every variant is down-weighted relative to the
//! query's real tokens by the caller — so a wrong guess costs nothing, while a right one
//! recovers a match Tantivy's tokenizer would otherwise never make.
//!
//! Tokens shorter than [`MIN_LEN`] and pure numbers get no variants (too short/generic to
//! guess safely), and non-ASCII tokens get none either (these heuristics don't apply, and
//! it keeps every byte index below ASCII-safe to slice on).

/// Tokens shorter than this get no variants at all — not enough signal to guess safely,
/// and it keeps every generated stem comfortably non-empty.
const MIN_LEN: usize = 4;

/// Cap on variants returned per token.
pub const MAX_VARIANTS: usize = 4;

/// Plausible morphological variants of `token`, excluding `token` itself, deduplicated,
/// capped at [`MAX_VARIANTS`]. `token` is expected to already be lowercase alphanumeric
/// (as [`super::tokenize`] produces) — this is not re-checked here.
pub fn variants(token: &str) -> Vec<String> {
    if token.len() < MIN_LEN || !token.is_ascii() || token.bytes().all(|b| b.is_ascii_digit()) {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for candidate in strip_candidates(token)
        .into_iter()
        .chain(add_candidates(token))
    {
        if candidate != token && !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out.truncate(MAX_VARIANTS);
    out
}

fn is_vowel(b: u8) -> bool {
    matches!(b, b'a' | b'e' | b'i' | b'o' | b'u')
}

fn is_consonant(b: u8) -> bool {
    b.is_ascii_alphabetic() && !is_vowel(b)
}

/// "index"/"release" style sibilant endings that pluralise with "-es" rather than a
/// plain "-s" (index -> indexes; church -> churches).
fn ends_sibilant(t: &str) -> bool {
    t.ends_with(['s', 'x', 'z']) || t.ends_with("ch") || t.ends_with("sh")
}

/// `t` ends in consonant + "y" (e.g. "quer|y", "entit|y") — the "-y" -> "-ies"
/// pluralisation branch, as opposed to a vowel + "y" ending (e.g. "day" -> "days").
fn consonant_y_stem(t: &str) -> Option<&str> {
    let b = t.as_bytes();
    let n = b.len();
    (n >= 2 && b[n - 1] == b'y' && is_consonant(b[n - 2])).then(|| &t[..n - 1])
}

/// Undo doubling of a final consonant before a stripped "-ing"/"-ed" suffix (e.g.
/// "stopp" -> "stop"). `None` when the last two characters aren't the same consonant.
fn undouble(stem: &str) -> Option<String> {
    let b = stem.as_bytes();
    let n = b.len();
    (n >= 2 && b[n - 1] == b[n - 2] && is_consonant(b[n - 1])).then(|| stem[..n - 1].to_string())
}

/// English's short-word CVC doubling rule for "-ing"/"-ed" (stop -> stopp-). Restricted
/// to short stems as a stand-in for "monosyllabic": this is a small, conservative helper
/// (not a syllable model), so an occasional miss on a longer word (e.g. "happen" ->
/// "happening", which this does not double for) is an acceptable, harmless gap.
fn double_final_consonant(t: &str) -> Option<String> {
    let b = t.as_bytes();
    let n = b.len();
    if !(3..=5).contains(&n) {
        return None;
    }
    let last = b[n - 1];
    if !is_consonant(last) || matches!(last, b'w' | b'x' | b'y') {
        return None;
    }
    if !is_vowel(b[n - 2]) {
        return None;
    }
    if is_vowel(b[n - 3]) {
        return None; // two vowels back-to-back = a long vowel sound: "read" -> "reading", not "readding"
    }
    Some(format!("{t}{}", last as char))
}

/// Assume `t` is already inflected; guess base-form candidates.
fn strip_candidates(t: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(stem) = t.strip_suffix("ies") {
        if stem.len() >= 2 {
            out.push(format!("{stem}y")); // entities -> entity, queries -> query
        }
    } else if let Some(stem) = t.strip_suffix("ing") {
        if stem.len() >= 3 {
            match undouble(stem) {
                Some(single) => out.push(single), // stopping -> stop
                None => {
                    out.push(format!("{stem}e")); // renaming -> rename
                    out.push(stem.to_string()); // renaming -> renam (harmless if wrong)
                }
            }
        }
    } else if let Some(stem) = t.strip_suffix("ed") {
        if stem.len() >= 3 {
            match undouble(stem) {
                Some(single) => out.push(single), // stopped -> stop
                None => {
                    out.push(format!("{stem}e")); // renamed -> rename
                    out.push(stem.to_string());
                }
            }
        }
    } else if let Some(stem) = t.strip_suffix("es") {
        // Two readings of the same "-es" ending: a sibilant plural (indexes -> index,
        // strip both letters) or a plain "-s" plural of a word already ending in "e"
        // (releases -> release, strip only the "s"). Try both; the wrong one is inert.
        if stem.len() >= 2 {
            out.push(stem.to_string()); // indexes -> index
        }
        let stem_s = &t[..t.len() - 1];
        if stem_s.len() >= 2 {
            out.push(stem_s.to_string()); // releases -> release
        }
    } else if !t.ends_with("ss")
        && let Some(stem) = t.strip_suffix('s')
        && stem.len() >= 2
    {
        out.push(stem.to_string()); // rules -> rule
    }
    out
}

/// Assume `t` is a base form; guess inflected candidates.
fn add_candidates(t: &str) -> Vec<String> {
    let mut out = Vec::new();
    // Plural.
    if let Some(stem) = consonant_y_stem(t) {
        out.push(format!("{stem}ies")); // entity -> entities, query -> queries
    } else if ends_sibilant(t) {
        out.push(format!("{t}es")); // index -> indexes
    } else {
        out.push(format!("{t}s")); // release -> releases, reference -> references
    }
    // Verb -ing / -ed.
    if let Some(doubled) = double_final_consonant(t) {
        out.push(format!("{doubled}ing")); // stop -> stopping
        out.push(format!("{doubled}ed")); // stop -> stopped
    } else if let Some(stem) = t.strip_suffix('e') {
        if !t.ends_with("ee") && stem.len() >= 2 {
            out.push(format!("{stem}ing")); // rename -> renaming, release -> releasing
            out.push(format!("{t}d")); // rename -> renamed, release -> released
        }
    } else {
        out.push(format!("{t}ing"));
        out.push(format!("{t}ed"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts `variants(word)` contains `expect` (order-independent; checked
    /// bidirectionally by the tests below).
    fn has(word: &str, expect: &str) {
        let v = variants(word);
        assert!(
            v.iter().any(|c| c == expect),
            "variants({word:?}) = {v:?}, expected to contain {expect:?}"
        );
    }

    #[test]
    fn entity_and_entities() {
        has("entity", "entities");
        has("entities", "entity");
    }

    #[test]
    fn reference_and_references() {
        has("reference", "references");
        has("references", "reference");
    }

    #[test]
    fn rename_renaming_renamed() {
        has("rename", "renaming");
        has("rename", "renamed");
        has("renaming", "rename");
        has("renamed", "rename");
    }

    #[test]
    fn release_and_releases() {
        has("release", "releases");
        has("releases", "release");
    }

    #[test]
    fn index_and_indexes() {
        has("index", "indexes");
        has("indexes", "index");
    }

    #[test]
    fn query_and_queries() {
        has("query", "queries");
        has("queries", "query");
    }

    #[test]
    fn stop_and_stopped() {
        has("stop", "stopped");
        has("stopped", "stop");
    }

    #[test]
    fn short_tokens_and_numbers_get_no_variants() {
        assert!(variants("cat").is_empty(), "3 chars, below MIN_LEN");
        assert!(variants("api").is_empty(), "3 chars, below MIN_LEN");
        assert!(variants("2024").is_empty(), "pure number");
        assert!(variants("v2").is_empty(), "2 chars, below MIN_LEN");
    }

    #[test]
    fn non_ascii_tokens_get_no_variants() {
        // Rust string slicing here assumes ASCII byte == char; guaranteed by this guard
        // rather than by hoping every caller only ever tokenizes ASCII text.
        assert!(variants("café").is_empty());
        assert!(variants("naïve").is_empty());
    }

    #[test]
    fn capped_at_max_variants() {
        for word in [
            "entity",
            "entities",
            "release",
            "releases",
            "rename",
            "renaming",
            "index",
            "indexes",
            "reference",
            "references",
        ] {
            let v = variants(word);
            assert!(v.len() <= MAX_VARIANTS, "{word}: {v:?}");
        }
    }

    #[test]
    fn never_includes_the_input_itself() {
        for word in [
            "entity", "entities", "release", "releases", "rename", "renaming", "renamed", "stop",
            "stopped", "index", "indexes", "query", "queries",
        ] {
            assert!(
                !variants(word).contains(&word.to_string()),
                "variants({word}) must not include {word} itself"
            );
        }
    }

    #[test]
    fn no_duplicate_variants() {
        for word in ["entity", "release", "rename", "index", "query", "stop"] {
            let v = variants(word);
            let mut deduped = v.clone();
            deduped.sort();
            deduped.dedup();
            assert_eq!(v.len(), deduped.len(), "{word}: {v:?} has duplicates");
        }
    }
}
