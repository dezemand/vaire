//! Embeddings — pluggable, local by default (design.md §9).
//!
//! Embedding is a single seam: `embed(texts) → vectors`. Local by default for three
//! reasons the spec locks in — confidential corpora (an API means data egress on every
//! section), offline/rebuild-in-seconds, and re-embed-on-every-reindex. A
//! [`cache::EmbedCache`] keyed by content hash makes reindex re-embed only changed
//! sections.
//!
//! Two providers (cli.md §6): the built-in `local` embedder, and `command`, which
//! shells out to a configured program (texts on stdin → vectors out).

pub mod cache;

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::config::EmbeddingProvider;
use crate::error::{Result, VaireError};
use crate::userconfig::UserConfig;

/// The one seam every embedding provider implements.
pub trait Embedder {
    /// Embed a batch of texts into fixed-dimension vectors. The returned outer length
    /// equals `texts.len()`; each inner vector has [`Embedder::dimensions`] elements.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    fn dimensions(&self) -> usize;

    /// A stable identity string for the provider — `provider[:model]:dims`, e.g.
    /// `openai:text-embedding-3-small:1536`, `local:384`. Recorded as index meta
    /// (`embed_provider`) at every build: the content-hash cache keys on section *text*
    /// only, so this identity is what guards against mixing vectors from different
    /// providers/models in one index (design.md §9).
    fn identity(&self) -> String {
        format!("unknown:{}", self.dimensions())
    }
}

/// Build the configured embedder from the global user config (M2). Secrets
/// (`OPENAI_API_KEY`, `OPENAI_BASE_URL`) resolve via [`crate::userconfig::credential`] — an
/// environment variable first, then `credentials.toml`.
pub fn from_user_config(user: &UserConfig) -> Result<Box<dyn Embedder>> {
    let emb = &user.embeddings;
    let dims = emb.dimensions;
    match emb.provider {
        EmbeddingProvider::Local => Ok(Box::new(LocalEmbedder { dims })),
        EmbeddingProvider::Command => Ok(Box::new(CommandEmbedder {
            command: emb.command.clone(),
            dims,
        })),
        EmbeddingProvider::OpenAi => {
            let api_key = crate::userconfig::credential("OPENAI_API_KEY").ok_or_else(|| {
                VaireError::Config(
                    "embeddings provider is \"openai\" but OPENAI_API_KEY is not set \
                     (env var or credentials.toml — run `vaire configure embeddings`)"
                        .into(),
                )
            })?;
            let base_url = validate_base_url(
                &crate::userconfig::credential("OPENAI_BASE_URL")
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            )?;
            Ok(Box::new(OpenAiEmbedder {
                api_key,
                base_url,
                model: emb.embedding_model.clone(),
                dims,
                agent: ureq::AgentBuilder::new()
                    .timeout(Duration::from_secs(30))
                    .build(),
            }))
        }
    }
}

/// The built-in, in-process embedder (no network, no model file).
///
/// Step-2 implementation: deterministic **feature hashing** — tokens are hashed into
/// `dims` buckets and the vector is L2-normalized. It is weak semantically (by design,
/// vectors are only the recall layer behind FTS + aliases — design.md §9), but it is
/// offline, dependency-free, and stable across runs, so `vaire index`/`search` work
/// today. Step 4 swaps in a real local model behind this same trait.
pub struct LocalEmbedder {
    dims: usize,
}

impl Embedder for LocalEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| feature_hash(t, self.dims)).collect())
    }
    fn dimensions(&self) -> usize {
        self.dims
    }
    fn identity(&self) -> String {
        format!("local:{}", self.dims)
    }
}

/// Bag-of-words feature hashing into a fixed-width, L2-normalized vector.
fn feature_hash(text: &str, dims: usize) -> Vec<f32> {
    use std::hash::{Hash, Hasher};
    let mut v = vec![0.0f32; dims.max(1)];
    for token in text.split(|c: char| !c.is_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        token.to_lowercase().hash(&mut h);
        let bucket = (h.finish() as usize) % v.len();
        v[bucket] += 1.0;
    }
    l2_normalize(&mut v);
    v
}

/// Shells out to a configured `command` (cli.md §6) — the plug point for a *real* local
/// model without baking a heavy ONNX dependency into `vaire`.
///
/// Wire protocol (newline-safe, since section bodies are multi-line):
/// - **stdin**: a single JSON array of strings — the texts to embed.
/// - **stdout**: a single JSON array of vectors (`[[f32, …], …]`), same length and order.
///
/// The command runs via `sh -c`, so it may include arguments/pipes (e.g. a
/// `sentence-transformers` script, `llama.cpp` embeddings, or an Ollama call). It must
/// read all of stdin and emit the JSON result on stdout.
pub struct CommandEmbedder {
    command: String,
    dims: usize,
}

impl Embedder for CommandEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        if self.command.trim().is_empty() {
            return Err(VaireError::Config(
                "embeddings.provider = \"command\" but embeddings.command is empty".into(),
            ));
        }

        let input = serde_json::to_vec(texts)
            .map_err(|e| VaireError::Config(format!("encode texts for embedding command: {e}")))?;

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&self.command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| VaireError::Config(format!("spawn embedding command: {e}")))?;

        // Write stdin from a thread so a large output can't deadlock against a full
        // stdin pipe.
        let mut stdin = child.stdin.take().expect("piped stdin");
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
        let output = child
            .wait_with_output()
            .map_err(|e| VaireError::Config(format!("embedding command io: {e}")))?;
        let _ = writer.join();

        if !output.status.success() {
            return Err(VaireError::Config(format!(
                "embedding command failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }

        let vectors: Vec<Vec<f32>> = serde_json::from_slice(&output.stdout).map_err(|e| {
            VaireError::Config(format!(
                "embedding command output is not a JSON array of vectors: {e}"
            ))
        })?;
        if vectors.len() != texts.len() {
            return Err(VaireError::Config(format!(
                "embedding command returned {} vectors for {} texts",
                vectors.len(),
                texts.len()
            )));
        }
        Ok(vectors)
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
    fn identity(&self) -> String {
        // The command string IS the model choice here — hash it (stable FNV-1a, not the
        // std hasher, since identities persist in index meta across binary versions), so
        // swapping the script behind `embeddings.command` changes the identity even when
        // the dimensionality happens to match.
        format!(
            "command:{:016x}:{}",
            fnv1a(self.command.as_bytes()),
            self.dims
        )
    }
}

/// Stable 64-bit FNV-1a — identity strings are persisted and compared across builds, so
/// the hash must never change between Rust/std versions.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Embeds via the OpenAI embeddings API (network). Opt-in (`provider = "openai"`) — it
/// means data egress per section, so it is never the default. The key comes from
/// `OPENAI_API_KEY` (env or `credentials.toml`); `OPENAI_BASE_URL` overrides the endpoint for
/// proxies/Azure-style gateways.
pub struct OpenAiEmbedder {
    api_key: String,
    base_url: String,
    model: String,
    dims: usize,
    /// Retains the HTTP connection pool across batched requests (especially useful for MCP).
    agent: ureq::Agent,
}

impl Embedder for OpenAiEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        // The API rejects empty strings, and section bodies can be empty (e.g. a heading
        // with no content). A blank text yields no pieces and gets a zero vector afterwards
        // (cosine treats it as non-matching).
        //
        // It also rejects any single input over 8192 tokens, which an oversized section (a
        // large table, a document with no subheadings) hits in practice. Rather than
        // truncating and losing the tail, a text over the cap is split into pieces that fit,
        // every piece is embedded, and the pieces are pooled back into one vector per text.
        let plan: Vec<Vec<&str>> = texts
            .iter()
            .map(|text| split_for_embedding(text, MAX_INPUT_BYTES))
            .collect();
        let pieces: Vec<&str> = plan.iter().flatten().copied().collect();
        if pieces.is_empty() {
            return Ok(vec![vec![0.0; self.dims]; texts.len()]);
        }

        let max_inputs = MAX_REQUEST_INPUTS.min(max_inputs_for_response(self.dims));
        let mut vectors = Vec::with_capacity(pieces.len());
        for request in request_batches(&pieces, max_inputs, MAX_REQUEST_BYTES) {
            vectors.extend(self.request(request)?);
        }
        pool_pieces(&plan, vectors, self.dims)
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn identity(&self) -> String {
        format!("openai:{}:{}", self.model, self.dims)
    }
}

impl OpenAiEmbedder {
    /// One embeddings API call. `inputs` must be non-blank and within the per-input and
    /// per-request limits — [`OpenAiEmbedder::embed`] arranges both.
    fn request(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut body = serde_json::json!({ "model": self.model, "input": inputs });
        // v3 models support dimension reduction; older models (e.g. ada-002) don't accept
        // the parameter, so only send it for those that do.
        if self.model.starts_with("text-embedding-3") {
            body["dimensions"] = serde_json::json!(self.dims);
        }
        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));
        let payload = serde_json::to_string(&body).expect("serialize embeddings request");

        let response = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .send_string(&payload)
            .map_err(|e| match e {
                ureq::Error::Status(code, resp) => VaireError::Config(format!(
                    "openai embeddings HTTP {code}: {}",
                    crate::output::style::plain(
                        read_response_limited(resp.into_reader())
                            .unwrap_or_else(|_| "<response body unavailable or too large>".into())
                            .trim()
                    )
                )),
                other => VaireError::Config(format!("openai embeddings request failed: {other}")),
            })?;
        let text = read_response_limited(response.into_reader())
            .map_err(|e| VaireError::Config(format!("openai embeddings: reading response: {e}")))?;
        parse_embedding_response(&text, inputs.len())
    }
}

const MAX_EMBEDDING_RESPONSE_BYTES: u64 = 10 * 1024 * 1024;

/// A single OpenAI embeddings input may not exceed 8192 tokens. The embeddings models use
/// byte-level BPE, where every token consumes at least one raw UTF-8 byte, so a piece of at
/// most this many bytes is under the token limit for any language or content, with no
/// tokenizer needed. The bound is conservative (English averages ~4 bytes per token), but
/// because oversized text is split rather than truncated, that costs an extra piece, never
/// content.
const MAX_INPUT_BYTES: usize = 8000;
/// The API also caps one request at 300,000 tokens summed across its inputs (bounded via
/// bytes, as above) and at 2048 inputs.
const MAX_REQUEST_BYTES: usize = 290_000;
const MAX_REQUEST_INPUTS: usize = 2048;

/// How many inputs one request may carry so that its response fits under
/// `MAX_EMBEDDING_RESPONSE_BYTES`. The response grows with inputs × `dims`, and the API
/// pretty-prints one float per line, so a float costs up to ~32 bytes (indentation, up to 22
/// characters, `,\n`) plus a little per vector for its `object`/`index` keys and brackets.
fn max_inputs_for_response(dims: usize) -> usize {
    const BYTES_PER_FLOAT: usize = 32;
    const BYTES_PER_VECTOR: usize = 128;
    let per_vector = dims
        .saturating_mul(BYTES_PER_FLOAT)
        .saturating_add(BYTES_PER_VECTOR);
    (MAX_EMBEDDING_RESPONSE_BYTES as usize / per_vector).max(1)
}

/// Split `text` into consecutive pieces of at most `max_bytes` bytes each. A cut prefers a
/// paragraph break, then a line break, then a space, and never lands inside a UTF-8
/// character. Blank pieces are dropped (the API rejects them), so a blank text yields none.
fn split_for_embedding(text: &str, max_bytes: usize) -> Vec<&str> {
    debug_assert!(
        max_bytes >= 4,
        "a piece must fit any single UTF-8 character"
    );
    let mut pieces = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let end = if rest.len() <= max_bytes {
            rest.len()
        } else {
            cut_point(rest, max_bytes)
        };
        let (piece, tail) = rest.split_at(end);
        if !piece.trim().is_empty() {
            pieces.push(piece);
        }
        rest = tail;
    }
    pieces
}

/// Where to end the next piece of `s` (longer than `max_bytes`). A natural break only counts
/// in the back half of the window, so a stray early newline can't shrink pieces to slivers.
fn cut_point(s: &str, max_bytes: usize) -> usize {
    let mut limit = max_bytes;
    while !s.is_char_boundary(limit) {
        limit -= 1;
    }
    let window = &s[..limit];
    for separator in ["\n\n", "\n", " "] {
        if let Some(at) = window.rfind(separator).filter(|&at| at >= limit / 2) {
            return at + separator.len();
        }
    }
    limit
}

/// Group pieces into consecutive requests of at most `max_inputs` inputs and `max_bytes`
/// bytes summed. Each piece is already within the per-input cap, which is far below
/// `max_bytes`, so every request holds at least one piece.
fn request_batches<'a>(
    pieces: &'a [&'a str],
    max_inputs: usize,
    max_bytes: usize,
) -> Vec<&'a [&'a str]> {
    let mut batches = Vec::new();
    let mut start = 0;
    let mut bytes = 0;
    for (i, piece) in pieces.iter().enumerate() {
        if i > start && (i - start == max_inputs || bytes + piece.len() > max_bytes) {
            batches.push(&pieces[start..i]);
            start = i;
            bytes = 0;
        }
        bytes += piece.len();
    }
    if start < pieces.len() {
        batches.push(&pieces[start..]);
    }
    batches
}

/// Pool per-piece vectors back into one vector per original text. `plan` holds each text's
/// pieces, in the order their vectors arrive. A text that fit whole keeps its vector as-is.
/// A split text gets the length-weighted mean of its pieces' vectors, re-normalized, which is
/// the approach OpenAI recommends for text longer than a model's context. A blank text has
/// no pieces and gets a zero vector, sized to match (falling back to `dim_fallback` if every
/// text was blank).
fn pool_pieces(
    plan: &[Vec<&str>],
    vectors: Vec<Vec<f32>>,
    dim_fallback: usize,
) -> Result<Vec<Vec<f32>>> {
    let dim = vectors.first().map(Vec::len).unwrap_or(dim_fallback);
    let mut vectors = vectors.into_iter();
    let mut next = || {
        vectors.next().ok_or_else(|| {
            VaireError::Config("openai embeddings: fewer vectors than input pieces".into())
        })
    };
    let mut out = Vec::with_capacity(plan.len());
    for pieces in plan {
        let vector = match pieces.as_slice() {
            [] => vec![0.0; dim],
            [_] => next()?,
            _ => {
                let mut pooled = vec![0.0f32; dim];
                for piece in pieces {
                    let weight = piece.len() as f32;
                    for (sum, x) in pooled.iter_mut().zip(next()?) {
                        *sum += weight * x;
                    }
                }
                l2_normalize(&mut pooled);
                pooled
            }
        };
        out.push(vector);
    }
    Ok(out)
}

/// Scale `v` to unit length in place; a zero vector is left as-is.
fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v {
            *x /= norm;
        }
    }
}

/// Only HTTPS endpoints are accepted, except explicit loopback development endpoints. This
/// prevents accidentally sending a bearer key and corpus text in cleartext to a proxy.
fn validate_base_url(value: &str) -> Result<String> {
    let url = url::Url::parse(value)
        .map_err(|e| VaireError::Config(format!("invalid OPENAI_BASE_URL: {e}")))?;
    let loopback = match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(VaireError::Config(
            "OPENAI_BASE_URL must use https (http is allowed only for localhost)".into(),
        ));
    }
    if url.host_str().is_none() {
        return Err(VaireError::Config(
            "OPENAI_BASE_URL must include a host".into(),
        ));
    }
    Ok(value.trim_end_matches('/').to_string())
}

fn read_response_limited(mut reader: impl Read) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_EMBEDDING_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_EMBEDDING_RESPONSE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "response exceeds 10 MiB limit",
        ));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[derive(serde::Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingDatum>,
}

#[derive(serde::Deserialize)]
struct EmbeddingDatum {
    embedding: Vec<f32>,
    #[serde(default)]
    index: usize,
}

/// Parse an OpenAI embeddings response body into vectors, ordered by `index`.
fn parse_embedding_response(body: &str, expected: usize) -> Result<Vec<Vec<f32>>> {
    let mut parsed: EmbeddingResponse = serde_json::from_str(body)
        .map_err(|e| VaireError::Config(format!("openai embeddings: unexpected response: {e}")))?;
    parsed.data.sort_by_key(|d| d.index);
    if parsed.data.len() != expected {
        return Err(VaireError::Config(format!(
            "openai embeddings returned {} vectors for {} inputs",
            parsed.data.len(),
            expected
        )));
    }
    Ok(parsed.data.into_iter().map(|d| d.embedding).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EmbeddingConfig;

    fn command_user_config(command: &str) -> UserConfig {
        UserConfig {
            embeddings: EmbeddingConfig {
                provider: EmbeddingProvider::Command,
                command: command.to_string(),
                ..EmbeddingConfig::default()
            },
            ..UserConfig::default()
        }
    }

    #[test]
    fn command_embedder_pipes_texts_and_parses_vectors() {
        // Reads (and ignores) the JSON on stdin, returns one vector per the two inputs.
        let emb = from_user_config(&command_user_config(
            "cat >/dev/null; printf '[[1.0,0.0],[0.0,1.0]]'",
        ))
        .unwrap();
        let out = emb
            .embed(&["alpha".to_string(), "beta".to_string()])
            .unwrap();
        assert_eq!(out, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    }

    #[test]
    fn command_embedder_rejects_wrong_vector_count() {
        let emb =
            from_user_config(&command_user_config("cat >/dev/null; printf '[[1.0,0.0]]'")).unwrap();
        let err = emb.embed(&["a".to_string(), "b".to_string()]).unwrap_err();
        assert!(err.to_string().contains("returned 1 vectors for 2 texts"));
    }

    #[test]
    fn command_embedder_reports_command_failure() {
        let emb = from_user_config(&command_user_config("exit 3")).unwrap();
        assert!(emb.embed(&["a".to_string()]).is_err());
    }

    #[test]
    fn openai_missing_key_is_a_clear_error() {
        // Only meaningful when no key is configured anywhere; skip otherwise to stay
        // deterministic (from_user_config reads the real credential source).
        if crate::userconfig::credential("OPENAI_API_KEY").is_some() {
            return;
        }
        let cfg = UserConfig {
            embeddings: EmbeddingConfig {
                provider: EmbeddingProvider::OpenAi,
                ..EmbeddingConfig::default()
            },
            ..UserConfig::default()
        };
        let result = from_user_config(&cfg);
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("OPENAI_API_KEY"));
    }

    #[test]
    fn empty_inputs_get_zero_vectors_and_keep_alignment() {
        let texts = ["a", "", "b", "   "];
        let plan: Vec<Vec<&str>> = texts
            .iter()
            .map(|t| split_for_embedding(t, MAX_INPUT_BYTES))
            .collect();
        // Embedder returns vectors only for the two non-empty inputs.
        let embedded = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        let out = pool_pieces(&plan, embedded, 2).unwrap();
        assert_eq!(
            out,
            vec![
                vec![1.0, 2.0],
                vec![0.0, 0.0], // "" → zero vector, sized to match
                vec![3.0, 4.0],
                vec![0.0, 0.0], // whitespace-only → zero vector
            ]
        );
    }

    #[test]
    fn openai_response_parses_in_index_order() {
        let body =
            r#"{"data":[{"embedding":[0.3,0.4],"index":1},{"embedding":[0.1,0.2],"index":0}]}"#;
        let vectors = parse_embedding_response(body, 2).unwrap();
        assert_eq!(vectors, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
        assert!(parse_embedding_response(body, 3).is_err());
    }

    #[test]
    fn embedding_url_requires_https_except_loopback() {
        assert!(validate_base_url("https://api.example/v1").is_ok());
        assert!(validate_base_url("http://localhost:11434/v1").is_ok());
        assert!(validate_base_url("http://[::1]:11434/v1").is_ok());
        assert!(validate_base_url("http://api.example/v1").is_err());
        assert!(validate_base_url("not a URL").is_err());
    }

    #[test]
    fn command_identity_tracks_the_command_not_just_dims() {
        // Two different scripts with the same dims must NOT share an identity — the
        // identity is what stops an index mixing vectors across a command swap.
        let a = CommandEmbedder {
            command: "model-a.sh".into(),
            dims: 384,
        };
        let b = CommandEmbedder {
            command: "model-b.sh".into(),
            dims: 384,
        };
        assert_ne!(a.identity(), b.identity());

        // Stable across instances (persisted in index meta, compared across runs).
        let a2 = CommandEmbedder {
            command: "model-a.sh".into(),
            dims: 384,
        };
        assert_eq!(a.identity(), a2.identity());
    }

    #[test]
    fn split_for_embedding_keeps_a_fitting_text_whole_and_drops_blank_ones() {
        assert_eq!(split_for_embedding("hello", 8000), vec!["hello"]);
        assert!(split_for_embedding("", 8000).is_empty());
        assert!(split_for_embedding(" \n ", 8000).is_empty());
    }

    #[test]
    fn split_for_embedding_covers_the_text_within_the_cap() {
        let text = format!("a\n{}", "b".repeat(50));
        let pieces = split_for_embedding(&text, 20);
        assert!(pieces.iter().all(|p| p.len() <= 20), "{pieces:?}");
        assert_eq!(pieces.concat(), text, "no content lost at the cuts");
        // The newline sits in the front half of the window, so it isn't taken as a cut.
        assert_eq!(pieces[0].len(), 20);
    }

    #[test]
    fn split_for_embedding_prefers_a_paragraph_break() {
        let text = format!("{}\n\n{}", "a".repeat(30), "b".repeat(30));
        let pieces = split_for_embedding(&text, 40);
        assert_eq!(
            pieces,
            vec![format!("{}\n\n", "a".repeat(30)), "b".repeat(30)]
        );
    }

    #[test]
    fn split_for_embedding_never_splits_a_multibyte_codepoint() {
        // Each "é" is 2 UTF-8 bytes; a cap landing mid-codepoint backs off to the boundary.
        let text = "é".repeat(10);
        let pieces = split_for_embedding(&text, 15);
        assert_eq!(pieces, vec!["é".repeat(7), "é".repeat(3)]);
    }

    #[test]
    fn request_batches_respect_input_and_byte_limits() {
        let pieces = ["aaaa", "bbbb", "cc", "d", "e", "f"];
        let batches = request_batches(&pieces, 3, 10);
        assert_eq!(
            batches,
            vec![&["aaaa", "bbbb", "cc"][..], &["d", "e", "f"][..]]
        );
        let batches = request_batches(&pieces, 10, 8);
        assert_eq!(
            batches,
            vec![&["aaaa", "bbbb"][..], &["cc", "d", "e", "f"][..]]
        );
    }

    #[test]
    fn a_full_request_keeps_its_response_under_the_size_cap() {
        for dims in [384, 1536, 3072] {
            let inputs = max_inputs_for_response(dims);
            assert!(inputs >= 1);
            assert!(
                inputs * (dims * 32 + 128) <= MAX_EMBEDDING_RESPONSE_BYTES as usize,
                "{dims} dims: {inputs} inputs could overflow the response cap"
            );
        }
        // 3072-dim vectors (text-embedding-3-large) no longer fit the indexer's 128-section
        // batches in one response, so those are split across requests.
        assert!(max_inputs_for_response(3072) < 128);
    }

    #[test]
    fn pool_pieces_takes_the_length_weighted_mean_of_a_split_text() {
        // Weights 3 and 1 over orthogonal unit vectors → (3, 1) normalized.
        let plan = vec![vec!["aaa", "b"], vec!["whole"]];
        let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![0.6, 0.8]];
        let out = pool_pieces(&plan, vectors, 2).unwrap();
        let norm = 10f32.sqrt();
        assert!((out[0][0] - 3.0 / norm).abs() < 1e-6 && (out[0][1] - 1.0 / norm).abs() < 1e-6);
        assert_eq!(
            out[1],
            vec![0.6, 0.8],
            "a text that fit whole keeps its vector"
        );
        assert!(pool_pieces(&plan, vec![vec![1.0, 0.0]], 2).is_err());
    }

    /// Serve OpenAI-shaped embeddings responses on loopback for `requests` calls, returning
    /// the inputs each call carried. An input starting with "b" embeds to `[0, 1]`, anything
    /// else to `[1, 0]`.
    fn mock_openai(requests: usize) -> (String, std::thread::JoinHandle<Vec<Vec<String>>>) {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for stream in listener.incoming().take(requests) {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let inputs: Vec<String> = serde_json::from_value(request["input"].clone()).unwrap();
                let data: Vec<_> = inputs
                    .iter()
                    .enumerate()
                    .map(|(index, input)| {
                        let embedding = if input.starts_with('b') {
                            [0.0, 1.0]
                        } else {
                            [1.0, 0.0]
                        };
                        serde_json::json!({ "embedding": embedding, "index": index })
                    })
                    .collect();
                let response = serde_json::json!({ "data": data }).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                )
                .unwrap();
                seen.push(inputs);
            }
            seen
        });
        (base_url, server)
    }

    #[test]
    fn openai_embed_splits_an_oversized_section_and_pools_it_to_one_vector() {
        // Regression for HTTP 400 "maximum input length is 8192 tokens": a section past the
        // per-input cap must reach the wire as pieces within the cap, and come back as one
        // vector carrying all of them rather than a truncated head.
        let (base_url, server) = mock_openai(1);
        let embedder = OpenAiEmbedder {
            api_key: "test".into(),
            base_url,
            model: "text-embedding-3-small".into(),
            dims: 2,
            agent: ureq::AgentBuilder::new().build(),
        };
        let oversized = format!("{}\n\n{}", "a".repeat(6000), "b".repeat(6000));
        let texts = vec!["short".to_string(), String::new(), oversized];

        let vectors = embedder.embed(&texts).unwrap();
        let requests = server.join().unwrap();

        let lengths: Vec<usize> = requests[0].iter().map(String::len).collect();
        assert_eq!(
            lengths,
            vec![5, 6002, 6000],
            "blank dropped, oversized split in two"
        );
        assert_eq!(vectors.len(), 3, "one vector per section, aligned");
        assert_eq!(vectors[0], vec![1.0, 0.0]);
        assert_eq!(vectors[1], vec![0.0, 0.0]);
        // Near-equal halves embedding to orthogonal vectors pool to their bisector.
        let half = std::f32::consts::FRAC_1_SQRT_2;
        assert!((vectors[2][0] - half).abs() < 1e-3 && (vectors[2][1] - half).abs() < 1e-3);
    }
}
