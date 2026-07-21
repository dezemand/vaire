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

use std::io::Write;
use std::process::{Command, Stdio};

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
            let base_url = crate::userconfig::credential("OPENAI_BASE_URL")
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
            Ok(Box::new(OpenAiEmbedder {
                api_key,
                base_url,
                model: emb.embedding_model.clone(),
                dims,
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
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
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
        format!("command:{}", self.dims)
    }
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
}

impl Embedder for OpenAiEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        // The API rejects empty strings, and section bodies can be empty (e.g. a heading
        // with no content). Send only the non-empty inputs; empty slots are filled with
        // zero vectors afterwards (cosine treats them as non-matching).
        let inputs: Vec<&str> = texts
            .iter()
            .map(String::as_str)
            .filter(|s| !s.trim().is_empty())
            .collect();
        if inputs.is_empty() {
            return Ok(vec![vec![0.0; self.dims]; texts.len()]);
        }

        let mut body = serde_json::json!({ "model": self.model, "input": inputs });
        // v3 models support dimension reduction; older models (e.g. ada-002) don't accept
        // the parameter, so only send it for those that do.
        if self.model.starts_with("text-embedding-3") {
            body["dimensions"] = serde_json::json!(self.dims);
        }
        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));
        let payload = serde_json::to_string(&body).expect("serialize embeddings request");

        let response = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .send_string(&payload)
            .map_err(|e| match e {
                ureq::Error::Status(code, resp) => VaireError::Config(format!(
                    "openai embeddings HTTP {code}: {}",
                    resp.into_string().unwrap_or_default().trim()
                )),
                other => VaireError::Config(format!("openai embeddings request failed: {other}")),
            })?;
        let text = response
            .into_string()
            .map_err(|e| VaireError::Config(format!("openai embeddings: reading response: {e}")))?;
        let embedded = parse_embedding_response(&text, inputs.len())?;
        expand_with_empty_slots(texts, embedded, self.dims)
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn identity(&self) -> String {
        format!("openai:{}:{}", self.model, self.dims)
    }
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

/// Re-expand a result to one vector per original input: `embedded` holds a vector for
/// each non-empty text (in order); each empty text gets a zero vector sized to match
/// (falling back to `dim_fallback` if every text was empty).
fn expand_with_empty_slots(
    texts: &[String],
    embedded: Vec<Vec<f32>>,
    dim_fallback: usize,
) -> Result<Vec<Vec<f32>>> {
    let dim = embedded.first().map(Vec::len).unwrap_or(dim_fallback);
    let mut filled = embedded.into_iter();
    let mut out = Vec::with_capacity(texts.len());
    for text in texts {
        if text.trim().is_empty() {
            out.push(vec![0.0; dim]);
        } else {
            out.push(filled.next().ok_or_else(|| {
                VaireError::Config("openai embeddings: fewer vectors than non-empty inputs".into())
            })?);
        }
    }
    Ok(out)
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
        };
        let result = from_user_config(&cfg);
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("OPENAI_API_KEY"));
    }

    #[test]
    fn empty_inputs_get_zero_vectors_and_keep_alignment() {
        let texts = vec![
            "a".to_string(),
            "".to_string(),
            "b".to_string(),
            "   ".to_string(),
        ];
        // Embedder returns vectors only for the two non-empty inputs.
        let embedded = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        let out = expand_with_empty_slots(&texts, embedded, 2).unwrap();
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
}
