//! Corpus materialization for the search benchmark: `public`, `external`, `scale`.
//!
//! Every corpus is written into a fresh tempdir (never a git repo — `vaire::index::build::run`
//! with `Mode::Full` reads the working tree straight from disk when the corpus root has no
//! `.git`, which is exactly what discovery + a plain `knowledge.toml` give us here) and is
//! indexed by the caller via the same `vaire::index::build::run(&repo, &config, embedder,
//! Mode::Full)` call regardless of which corpus produced it.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use super::queries::Query;

/// A materialized corpus: a tempdir (kept alive for the caller's lifetime) plus whatever
/// human-readable notices its construction produced (e.g. "no authored nodes yet").
pub struct CorpusBuild {
    pub root: PathBuf,
    pub notices: Vec<String>,
    _tempdir: tempfile::TempDir,
}

impl CorpusBuild {
    fn new(tempdir: tempfile::TempDir, notices: Vec<String>) -> Self {
        let root = tempdir.path().to_path_buf();
        CorpusBuild {
            root,
            notices,
            _tempdir: tempdir,
        }
    }

    pub fn repo(&self) -> vaire::corpus::Repo {
        vaire::corpus::Repo::discover(Some(&self.root), &self.root)
            .expect("freshly-written corpus root carries its own knowledge.toml")
    }

    pub fn config(&self) -> vaire::config::Config {
        vaire::config::Config::load(&self.root.join("knowledge.toml"))
            .expect("freshly-written knowledge.toml parses")
    }
}

// ---------------------------------------------------------------------------------
// public
// ---------------------------------------------------------------------------------

/// `(source path relative to the vaire repo root, node id, display name, dest path
/// relative to the corpus root)` for every generated `document` node.
const DOCUMENT_SOURCES: &[(&str, &str, &str, &str)] = &[
    (
        "spec/design.md",
        "design-spec",
        "Design spec",
        "generated/documents/design-spec.md",
    ),
    (
        "spec/cli.md",
        "cli-spec",
        "CLI spec",
        "generated/documents/cli-spec.md",
    ),
    (
        "spec/registry.md",
        "registry-spec",
        "Registry spec",
        "generated/documents/registry-spec.md",
    ),
    (
        "spec/manifest.md",
        "manifest-spec",
        "Manifest spec",
        "generated/documents/manifest-spec.md",
    ),
    (
        "README.md",
        "readme",
        "README",
        "generated/documents/readme.md",
    ),
    (
        "CHANGELOG.md",
        "changelog",
        "Changelog",
        "generated/documents/changelog.md",
    ),
];

const PUBLIC_MANIFEST: &str = r#"name = "search-bench"
version = "0.1.0"
include = ["**/*.md"]
types = ["cli", "command", "concept", "principle", "decision", "component", "finding", "skill", "document", "guide", "roadmap"]
scoped_types_whitelist = ["command"]
"#;

/// The `public` corpus's hand-authored nodes, relative to the vaire repo root: one archive
/// written by [`pack_md_tree`] instead of a directory of Markdown files, so the fixture is a
/// single file in version control rather than ~90 diffs. `--unpack-corpus`/`--pack-corpus`
/// move it to a directory to edit and back (README, "Editing the public corpus").
pub const PUBLIC_NODES_ARCHIVE: &str = "benches/search/data/public/nodes.tar.gz";

/// Build the `public` corpus: authored fixture nodes (unpacked from
/// [`PUBLIC_NODES_ARCHIVE`]) plus generated `document` nodes from this repo's own long-form
/// docs and `skill` nodes from `skills/*/SKILL.md` — the adversarial mix issue #52 is about
/// (long specs vs. short concepts/skills).
///
/// `nodes_dir_override` lets a caller (the smoke-test acceptance path) point `nodes/` at an
/// arbitrary directory instead of the archive; `main.rs` never uses it for a real run.
pub fn build_public(
    vaire_repo_root: &Path,
    nodes_dir_override: Option<&Path>,
) -> Result<CorpusBuild, String> {
    let tempdir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let root = tempdir.path().to_path_buf();
    let mut notices = Vec::new();

    let nodes_dest = root.join("nodes");
    let authored = match nodes_dir_override {
        Some(dir) if dir.is_dir() => copy_md_tree(dir, &nodes_dest)?,
        Some(_) => 0,
        None => {
            let archive = vaire_repo_root.join(PUBLIC_NODES_ARCHIVE);
            if archive.is_file() {
                unpack_md_archive(&archive, &nodes_dest)?
            } else {
                0
            }
        }
    };
    if authored == 0 {
        notices.push(
            "public corpus: no authored .md nodes found — building from generated \
             documents/skills only"
                .to_string(),
        );
    }

    let mut generated_docs = 0usize;
    for (src_rel, id, name, dest_rel) in DOCUMENT_SOURCES {
        let src_path = vaire_repo_root.join(src_rel);
        let Ok(text) = std::fs::read_to_string(&src_path) else {
            notices.push(format!(
                "public corpus: source file {src_rel} not found, skipping document:{id}"
            ));
            continue;
        };
        let body = strip_leading_frontmatter(&text);
        write_frontmatter_node(&root, dest_rel, id, "document", name, body)
            .map_err(|e| format!("writing generated document:{id}: {e}"))?;
        generated_docs += 1;
    }

    let mut generated_skills = 0usize;
    let skills_root = vaire_repo_root.join("skills");
    if let Ok(entries) = std::fs::read_dir(&skills_root) {
        let mut dirs: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        dirs.sort(); // deterministic corpus regardless of readdir order
        for dir in dirs {
            let skill_md = skills_root.join(&dir).join("SKILL.md");
            let Ok(text) = std::fs::read_to_string(&skill_md) else {
                continue;
            };
            let body = strip_leading_frontmatter(&text);
            write_frontmatter_node(
                &root,
                &format!("generated/skills/{dir}.md"),
                &dir,
                "skill",
                &dir,
                body,
            )
            .map_err(|e| format!("writing generated skill:{dir}: {e}"))?;
            generated_skills += 1;
        }
    }

    notices.push(format!(
        "public corpus: {authored} authored node(s), {generated_docs} generated document(s), \
         {generated_skills} generated skill(s)"
    ));

    std::fs::write(root.join("knowledge.toml"), PUBLIC_MANIFEST)
        .map_err(|e| format!("writing knowledge.toml: {e}"))?;

    Ok(CorpusBuild::new(tempdir, notices))
}

/// Strip a single leading `---`-fenced YAML block (a generic doc frontmatter, e.g.
/// `spec/design.md`'s `title:`/`status:`/`date:` block — not a vaire node frontmatter),
/// returning the remainder verbatim. Files with no leading fence (or an unterminated one)
/// are returned unchanged.
fn strip_leading_frontmatter(content: &str) -> &str {
    let content_no_bom = content.strip_prefix('\u{feff}').unwrap_or(content);
    let Some(after_open) = content_no_bom
        .strip_prefix("---\r\n")
        .or_else(|| content_no_bom.strip_prefix("---\n"))
    else {
        return content_no_bom;
    };
    let mut offset = 0usize;
    for line in after_open.split_inclusive('\n') {
        offset += line.len();
        if line.trim_end_matches(['\n', '\r']) == "---" {
            return &after_open[offset..];
        }
    }
    // No closing fence — not actually a frontmatter block; return the original.
    content_no_bom
}

/// Write `<root>/<rel>` with vaire frontmatter (`id`, `type`, `name`) followed by `body`.
fn write_frontmatter_node(
    root: &Path,
    rel: &str,
    id: &str,
    node_type: &str,
    name: &str,
    body: &str,
) -> io::Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name_escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
    let content =
        format!("---\nid: {id}\ntype: {node_type}\nname: \"{name_escaped}\"\n---\n{body}");
    std::fs::write(path, content)
}

/// Copy every `.md` file under `src` into `dest`, preserving relative paths. Returns how
/// many files were copied (`0` if `src` has none).
fn copy_md_tree(src: &Path, dest: &Path) -> Result<usize, String> {
    let files = walk_files(src).map_err(|e| format!("walking {}: {e}", src.display()))?;
    let mut count = 0;
    for path in files {
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let rel = path.strip_prefix(src).expect("walked under src");
        let dest_path = dest.join(rel);
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&path, &dest_path)
            .map_err(|e| format!("copying {} -> {}: {e}", path.display(), dest_path.display()))?;
        count += 1;
    }
    Ok(count)
}

/// Every regular file under `dir`, recursively, sorted (symlinks are skipped — a corpus
/// fixture has no business containing one, and this avoids any cycle risk).
fn walk_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                out.push(entry.path());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Pack every `.md` file under `src` into a gzipped tar at `archive`, each entry named by its
/// `/`-separated path relative to `src`. Deterministic, like `vaire pack`: entries in sorted
/// order with pinned metadata (mode 0644, uid/gid 0, mtime 0) under a gzip stream with no
/// timestamp and a pinned level — the same files always give the same bytes, so packing an
/// unchanged tree leaves no diff. Returns how many files were packed; refuses a tree with
/// none, which is far more likely a wrong path than an intentionally empty corpus.
pub fn pack_md_tree(src: &Path, archive: &Path) -> Result<usize, String> {
    let files = walk_files(src).map_err(|e| format!("walking {}: {e}", src.display()))?;
    let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for path in files {
        if !is_packable_md(&path) {
            continue;
        }
        let rel = path.strip_prefix(src).expect("walked under src");
        let name = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let bytes = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        entries.insert(name, bytes);
    }
    if entries.is_empty() {
        return Err(format!("no .md files under {} to pack", src.display()));
    }

    if let Some(parent) = archive.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::create(archive)
        .map_err(|e| format!("creating {}: {e}", archive.display()))?;
    // Pinned, not `default()`: the compression level is part of "same bytes".
    let gz = flate2::GzBuilder::new()
        .mtime(0)
        .write(file, flate2::Compression::new(6));
    let mut tar = tar::Builder::new(gz);
    for (name, bytes) in &entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        // `append_data` sets the path (GNU long names included) and the checksum.
        tar.append_data(&mut header, name, bytes.as_slice())
            .map_err(|e| format!("packing {name}: {e}"))?;
    }
    let gz = tar
        .into_inner()
        .map_err(|e| format!("finishing {}: {e}", archive.display()))?;
    gz.finish()
        .map_err(|e| format!("finishing {}: {e}", archive.display()))?;
    Ok(entries.len())
}

/// Unpack the `.md` entries of an archive written by [`pack_md_tree`] into `dest`, returning
/// how many were written. Only regular files at plain relative paths are accepted: an
/// absolute path or a `..` would write outside `dest`, and a link is an escape that survives
/// extraction. Directory entries are skipped (parents are created as needed).
pub fn unpack_md_archive(archive: &Path, dest: &Path) -> Result<usize, String> {
    let file =
        std::fs::File::open(archive).map_err(|e| format!("opening {}: {e}", archive.display()))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let entries = tar
        .entries()
        .map_err(|e| format!("reading {}: {e}", archive.display()))?;
    let mut count = 0;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("reading {}: {e}", archive.display()))?;
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|e| format!("reading an entry name in {}: {e}", archive.display()))?
            .into_owned();
        let plain = !path.as_os_str().is_empty()
            && path
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)));
        if !kind.is_file() || !plain {
            return Err(format!(
                "{}: refusing entry {} (only regular files at plain relative paths)",
                archive.display(),
                path.display()
            ));
        }
        if !is_packable_md(&path) {
            continue;
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes)
            .map_err(|e| format!("reading {} from {}: {e}", path.display(), archive.display()))?;
        let target = dest.join(&path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&target, bytes).map_err(|e| format!("writing {}: {e}", target.display()))?;
        count += 1;
    }
    Ok(count)
}

/// A Markdown file that belongs in the corpus archive: a `.md` extension, and not a macOS
/// `._*` AppleDouble sidecar (which `tar` on macOS writes next to the files it archives).
fn is_packable_md(path: &Path) -> bool {
    let is_md = path.extension().and_then(|e| e.to_str()) == Some("md");
    let sidecar = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("._"));
    is_md && !sidecar
}

// ---------------------------------------------------------------------------------
// external — your own corpus, never checked in (`--external-dir` / `--external-queries`)
// ---------------------------------------------------------------------------------

/// Build the `external` corpus from a directory you provide: your own Vairë package, with
/// its own `knowledge.toml`, copied into a tempdir (excluding `.git/`, `.vaire/`,
/// `node_modules/`) and never written to. Use this for any corpus you don't want checked
/// into this repo — a private/internal knowledge base, a client corpus, anything with its
/// own judgments file you keep outside version control.
pub fn build_external(source_dir: &Path) -> Result<CorpusBuild, String> {
    if !source_dir.is_dir() {
        return Err(format!(
            "--external-dir {} is not a directory",
            source_dir.display()
        ));
    }
    let tempdir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let root = tempdir.path().to_path_buf();
    copy_tree_excluding(source_dir, &root, &[".git", ".vaire", "node_modules"])
        .map_err(|e| format!("copying --external-dir into a tempdir: {e}"))?;
    if !root.join("knowledge.toml").is_file() {
        return Err(
            "--external-dir has no knowledge.toml at its root (not a vaire package)".to_string(),
        );
    }
    Ok(CorpusBuild::new(tempdir, Vec::new()))
}

fn copy_tree_excluding(src: &Path, dest: &Path, exclude_dirnames: &[&str]) -> io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if exclude_dirnames
                .iter()
                .any(|ex| entry.file_name().to_str() == Some(ex))
            {
                continue;
            }
            copy_tree_excluding(&entry.path(), &dest.join(&name), exclude_dirnames)?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), dest.join(&name))?;
        }
        // Symlinks are deliberately skipped: never follow a link out of the source package.
    }
    Ok(())
}

// ---------------------------------------------------------------------------------
// split — one corpus spread over several packages (`--packages N`)
// ---------------------------------------------------------------------------------

/// Spread `build`'s Markdown files over `parts` separate packages, so a search across
/// several packages can be scored against the same judgments as the whole corpus. Each file
/// goes to the part a stable hash of its corpus-relative path picks (so every run splits the
/// same way), and each part gets the corpus's own manifest under its own name (`<name>-p0`,
/// `<name>-p1`, …). Ids stay unique across the parts because they were unique in the whole.
pub fn split(build: &CorpusBuild, parts: usize) -> Result<Vec<CorpusBuild>, String> {
    let parts = parts.max(1);
    let manifest = std::fs::read_to_string(build.root.join("knowledge.toml"))
        .map_err(|e| format!("reading the corpus manifest: {e}"))?;
    let base_name = build.config().name;

    let mut dirs = Vec::with_capacity(parts);
    for i in 0..parts {
        let tempdir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
        std::fs::write(
            tempdir.path().join("knowledge.toml"),
            rename_manifest(&manifest, &format!("{base_name}-p{i}")),
        )
        .map_err(|e| format!("writing knowledge.toml for part {i}: {e}"))?;
        dirs.push(tempdir);
    }

    let files = walk_files(&build.root).map_err(|e| format!("walking the corpus: {e}"))?;
    let mut counts = vec![0usize; parts];
    for path in files {
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let rel = path
            .strip_prefix(&build.root)
            .expect("walked under the root");
        let part = (fnv1a(rel.to_string_lossy().as_bytes()) % parts as u64) as usize;
        let dest = dirs[part].path().join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&path, &dest)
            .map_err(|e| format!("copying {} -> {}: {e}", path.display(), dest.display()))?;
        counts[part] += 1;
    }

    Ok(dirs
        .into_iter()
        .enumerate()
        .map(|(i, dir)| {
            CorpusBuild::new(
                dir,
                vec![format!("split part {i} of {parts}: {} file(s)", counts[i])],
            )
        })
        .collect())
}

/// `manifest` with its `name = …` line replaced by `name = "<name>"`.
fn rename_manifest(manifest: &str, name: &str) -> String {
    let mut out: String = manifest
        .lines()
        .map(|line| {
            let key = line.split('=').next().unwrap_or("").trim();
            if key == "name" {
                format!("name = \"{name}\"")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

/// FNV-1a, 64-bit: a hash whose value never changes between runs or Rust releases (std's
/// `DefaultHasher` promises neither), which is what keeps the split reproducible.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

// ---------------------------------------------------------------------------------
// scale — deterministic synthetic corpus for latency (and the issue #52 adversarial case)
// ---------------------------------------------------------------------------------

/// Fixed seed: every `scale` run (any machine, any branch) generates byte-identical text.
pub const SCALE_SEED: u64 = 0x5EED_1E5A_11A5_5CA1;

const VOCAB_SIZE: usize = 20_000;
const ZIPF_EXPONENT: f64 = 1.0;
const STOPWORD_PROB: f64 = 0.35;
const N_NEEDLES: usize = 30;
const NEEDLE_DOCS_PER_NEEDLE: usize = 3;
const N_LATENCY_QUERIES: usize = 30;

const STOPWORDS: &[&str] = &[
    "the", "of", "and", "a", "to", "in", "is", "you", "that", "it", "he", "was", "for", "on",
    "are", "as", "with", "his", "they", "i", "at", "be", "this", "have", "from", "or", "one",
    "had", "by", "word", "but", "not", "what", "all", "were", "we", "when", "your", "can", "said",
];

const ONSETS: &[&str] = &[
    "b", "c", "d", "f", "g", "h", "j", "k", "l", "m", "n", "p", "r", "s", "t", "v", "w", "z", "th",
    "sh", "ch", "br", "cr", "dr", "fr", "gr", "pr", "tr", "st", "sl", "sp", "gl", "pl", "cl", "bl",
];
const VOWELS: &[&str] = &["a", "e", "i", "o", "u", "ai", "ea", "oo", "ou", "ie"];
const CODAS: &[&str] = &["", "n", "r", "s", "t", "d", "m", "l", "ng", "k"];

/// A small, fully deterministic PRNG: SplitMix64 derives the seed state, xorshift64* is the
/// stream generator. No external `rand` dependency — this benchmark is std-only.
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut sm = seed;
        let mut splitmix_next = move || {
            sm = sm.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = sm;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        let mut state = splitmix_next();
        if state == 0 {
            state = 0x9E37_79B9_7F4A_7C15; // xorshift's one forbidden state
        }
        Rng { state }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// A uniform value in `0..n` (`0` when `n == 0`, so callers on possibly-empty
    /// collections never panic).
    pub fn gen_range(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
}

/// A cumulative-weight table for Zipf-like sampling over `0..n` by rank (rank 0 is most
/// frequent).
struct ZipfSampler {
    cumulative: Vec<f64>,
    total: f64,
}

impl ZipfSampler {
    fn new(n: usize, exponent: f64) -> Self {
        let mut cumulative = Vec::with_capacity(n);
        let mut acc = 0.0;
        for rank in 1..=n.max(1) {
            acc += 1.0 / (rank as f64).powf(exponent);
            cumulative.push(acc);
        }
        ZipfSampler {
            cumulative,
            total: acc,
        }
    }

    fn sample(&self, rng: &mut Rng) -> usize {
        let target = rng.next_f64() * self.total;
        match self
            .cumulative
            .binary_search_by(|c| c.partial_cmp(&target).unwrap_or(std::cmp::Ordering::Equal))
        {
            Ok(i) => i,
            Err(i) => i.min(self.cumulative.len() - 1),
        }
    }
}

/// The synthetic vocabulary: pronounceable words, Zipf-sampled by rank.
struct Vocab {
    words: Vec<String>,
    zipf: ZipfSampler,
}

impl Vocab {
    fn generate(rng: &mut Rng, size: usize) -> Self {
        let mut seen = HashSet::with_capacity(size * 2);
        let mut words = Vec::with_capacity(size);
        let mut attempts = 0usize;
        while words.len() < size && attempts < size * 50 {
            attempts += 1;
            let w = gen_pronounceable_word(rng);
            if w.len() >= 3 && seen.insert(w.clone()) {
                words.push(w);
            }
        }
        let zipf = ZipfSampler::new(words.len(), ZIPF_EXPONENT);
        Vocab { words, zipf }
    }

    /// One Zipf-weighted word.
    fn sample<'a>(&'a self, rng: &mut Rng) -> &'a str {
        &self.words[self.zipf.sample(rng)]
    }

    /// A uniformly-chosen word from the *mid-frequency* rank band — common enough to read
    /// as ordinary vocabulary, not so common it collides with filler noise. Used for needle
    /// phrases, which must stay distinctive.
    fn sample_mid_frequency(&self, rng: &mut Rng) -> &str {
        let lo = 200.min(self.words.len().saturating_sub(1));
        let hi = 3000.min(self.words.len()).max(lo + 1);
        &self.words[lo + rng.gen_range(hi - lo)]
    }
}

fn gen_pronounceable_word(rng: &mut Rng) -> String {
    let syllables = 1 + rng.gen_range(3); // 1..=3
    let mut w = String::new();
    for i in 0..syllables {
        w.push_str(ONSETS[rng.gen_range(ONSETS.len())]);
        w.push_str(VOWELS[rng.gen_range(VOWELS.len())]);
        if i + 1 == syllables || rng.gen_range(4) == 0 {
            w.push_str(CODAS[rng.gen_range(CODAS.len())]);
        }
    }
    w
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// A run of Zipf/stopword-mixed words, formatted as sentences of ~6-14 words.
fn gen_body_words(rng: &mut Rng, vocab: &Vocab, n_words: usize) -> Vec<String> {
    (0..n_words)
        .map(|_| {
            if rng.next_f64() < STOPWORD_PROB {
                STOPWORDS[rng.gen_range(STOPWORDS.len())].to_string()
            } else {
                vocab.sample(rng).to_string()
            }
        })
        .collect()
}

fn words_to_prose(rng: &mut Rng, words: &[String]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < words.len() {
        let len = 6 + rng.gen_range(9); // 6..=14
        let end = (i + len).min(words.len());
        let mut sentence = words[i..end].join(" ");
        if let Some(first_byte_len) = sentence.chars().next().map(char::len_utf8) {
            let (head, tail) = sentence.split_at(first_byte_len);
            sentence = head.to_uppercase() + tail;
        }
        out.push_str(&sentence);
        out.push_str(". ");
        i = end;
    }
    out.trim_end().to_string()
}

struct GeneratedNode {
    id: String,
    node_type: &'static str,
    name: String,
    rel_path: String,
    /// `(heading, body)` per `##` section.
    sections: Vec<(String, String)>,
}

fn write_generated_node(root: &Path, node: &GeneratedNode) -> io::Result<()> {
    let mut body = String::new();
    for (heading, text) in &node.sections {
        body.push_str("## ");
        body.push_str(heading);
        body.push_str("\n\n");
        body.push_str(text);
        body.push_str("\n\n");
    }
    write_frontmatter_node(
        root,
        &node.rel_path,
        &node.id,
        node.node_type,
        &node.name,
        &body,
    )
}

/// A short concept node: 1-3 sections, ~50-300 words total.
fn gen_concept(rng: &mut Rng, vocab: &Vocab, idx: usize) -> GeneratedNode {
    let n_sections = 1 + rng.gen_range(3);
    let total_words = 50 + rng.gen_range(251);
    let words_per_section = (total_words / n_sections).max(5);
    let name = (0..2 + rng.gen_range(2))
        .map(|_| capitalize(vocab.sample(rng)))
        .collect::<Vec<_>>()
        .join(" ");
    let sections = (0..n_sections)
        .map(|_| {
            let heading = capitalize(vocab.sample(rng));
            let words = gen_body_words(rng, vocab, words_per_section);
            (heading, words_to_prose(rng, &words))
        })
        .collect();
    GeneratedNode {
        id: format!("c-{idx:06}"),
        node_type: "concept",
        name,
        rel_path: format!("generated/scale/concepts/c-{idx:06}.md"),
        sections,
    }
}

/// A long document node: 15-40 sections, each independently ~200-600 words — the "long
/// spec document" side of issue #52.
fn gen_document(rng: &mut Rng, vocab: &Vocab, idx: usize) -> GeneratedNode {
    let n_sections = 15 + rng.gen_range(26);
    let name = (0..2 + rng.gen_range(3))
        .map(|_| capitalize(vocab.sample(rng)))
        .collect::<Vec<_>>()
        .join(" ");
    let sections = (0..n_sections)
        .map(|_| {
            let heading = capitalize(vocab.sample(rng));
            let word_count = 200 + rng.gen_range(401);
            let words = gen_body_words(rng, vocab, word_count);
            (heading, words_to_prose(rng, &words))
        })
        .collect();
    GeneratedNode {
        id: format!("d-{idx:04}"),
        node_type: "document",
        name,
        rel_path: format!("generated/scale/documents/d-{idx:04}.md"),
        sections,
    }
}

/// A planted "needle": a short concept node whose `name` is a distinctive 2-3 word
/// mid-frequency phrase, repeated several times in its own (short) body. Returns the node
/// plus the phrase's words (lowercase, as generated) for query text + adversarial injection.
fn gen_needle(
    rng: &mut Rng,
    vocab: &Vocab,
    idx: usize,
    used_phrases: &mut HashSet<String>,
) -> (GeneratedNode, Vec<String>) {
    let phrase_words = loop {
        let n = 2 + rng.gen_range(2); // 2..=3
        let words: Vec<String> = (0..n)
            .map(|_| vocab.sample_mid_frequency(rng).to_string())
            .collect();
        let key = words.join(" ");
        if used_phrases.insert(key) {
            break words;
        }
    };
    let name = phrase_words
        .iter()
        .map(|w| capitalize(w))
        .collect::<Vec<_>>()
        .join(" ");

    let n_sections = 1 + rng.gen_range(3);
    let total_words = 50 + rng.gen_range(251);
    let words_per_section = (total_words / n_sections).max(20);
    let sections = (0..n_sections)
        .map(|s| {
            let mut words = gen_body_words(rng, vocab, words_per_section);
            // Repeat the phrase several times within the section — the needle signal a
            // short, relevant node should win on, once ranking stops just summing raw term
            // counts over a whole (possibly huge) file.
            let repeats = 4 + rng.gen_range(5); // 4..=8
            for _ in 0..repeats {
                let pos = rng.gen_range(words.len() + 1);
                for (k, w) in phrase_words.iter().enumerate() {
                    let at = (pos + k).min(words.len());
                    words.insert(at, w.clone());
                }
            }
            let heading = if s == 0 {
                name.clone()
            } else {
                capitalize(vocab.sample(rng))
            };
            (heading, words_to_prose(rng, &words))
        })
        .collect();

    (
        GeneratedNode {
            id: format!("needle-{idx:02}"),
            node_type: "concept",
            name,
            rel_path: format!("generated/scale/needles/needle-{idx:02}.md"),
            sections,
        },
        phrase_words,
    )
}

/// Inject a needle's phrase words many times, spread across a few randomly-chosen long
/// documents — the adversarial half of issue #52: raw term-count summed over every section
/// of a (large) file can dwarf a short, genuinely relevant concept's score.
fn inject_needle_into_documents(
    rng: &mut Rng,
    documents: &mut [GeneratedNode],
    phrase_words: &[String],
) {
    if documents.is_empty() {
        return;
    }
    let mut chosen: HashSet<usize> = HashSet::new();
    let target = NEEDLE_DOCS_PER_NEEDLE.min(documents.len());
    while chosen.len() < target {
        chosen.insert(rng.gen_range(documents.len()));
    }
    // `HashSet`'s iteration order is randomized per-instance (not just per-process) — every
    // build must be deterministic, and the loop below both mutates by index *and* advances
    // `rng`, so the order visited has to be fixed independent of hashing.
    let mut chosen: Vec<usize> = chosen.into_iter().collect();
    chosen.sort_unstable();
    let phrase = phrase_words.join(" ");
    for di in chosen {
        let doc = &mut documents[di];
        if doc.sections.is_empty() {
            continue;
        }
        let occurrences = 15 + rng.gen_range(16); // 15..=30
        for _ in 0..occurrences {
            let si = rng.gen_range(doc.sections.len());
            let body = &mut doc.sections[si].1;
            body.push(' ');
            body.push_str(&phrase);
            body.push('.');
        }
    }
}

const SCALE_MANIFEST: &str = "name = \"search-bench-scale\"\nversion = \"0.1.0\"\ninclude = [\"**/*.md\"]\ntypes = [\"concept\", \"document\"]\n";

/// Build the `scale` corpus: `n_nodes` synthetic nodes (~85% short concepts including 30
/// planted needles, ~15% long documents), plus the queries the plant implies (one `name`
/// query per needle, and `N_LATENCY_QUERIES` unjudged random-text queries).
pub fn build_scale(n_nodes: usize, seed: u64) -> Result<(CorpusBuild, Vec<Query>), String> {
    let n_nodes = n_nodes.max(1);
    let tempdir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let root = tempdir.path().to_path_buf();
    let mut rng = Rng::new(seed);

    let vocab = Vocab::generate(&mut rng, VOCAB_SIZE);

    let n_needles = N_NEEDLES.min(n_nodes);
    let n_documents = ((n_nodes * 15) / 100).clamp(1, n_nodes.saturating_sub(n_needles).max(1));
    let n_concepts_total = n_nodes.saturating_sub(n_documents).max(n_needles);
    let n_filler_concepts = n_concepts_total.saturating_sub(n_needles);

    let mut documents: Vec<GeneratedNode> = (0..n_documents)
        .map(|i| gen_document(&mut rng, &vocab, i))
        .collect();

    let mut used_phrases = HashSet::new();
    let mut needles = Vec::with_capacity(n_needles);
    let mut needle_phrases = Vec::with_capacity(n_needles);
    for i in 0..n_needles {
        let (node, phrase) = gen_needle(&mut rng, &vocab, i, &mut used_phrases);
        needles.push(node);
        needle_phrases.push(phrase);
    }
    for phrase in &needle_phrases {
        inject_needle_into_documents(&mut rng, &mut documents, phrase);
    }

    let filler: Vec<GeneratedNode> = (0..n_filler_concepts)
        .map(|i| gen_concept(&mut rng, &vocab, i))
        .collect();

    for node in documents.iter().chain(needles.iter()).chain(filler.iter()) {
        write_generated_node(&root, node).map_err(|e| format!("writing {}: {e}", node.id))?;
    }

    std::fs::write(root.join("knowledge.toml"), SCALE_MANIFEST)
        .map_err(|e| format!("writing knowledge.toml: {e}"))?;

    let mut queries = Vec::with_capacity(n_needles + N_LATENCY_QUERIES);
    for (node, phrase) in needles.iter().zip(&needle_phrases) {
        let mut relevant = BTreeMap::new();
        relevant.insert(format!("concept:{}", node.id), 2u8);
        queries.push(Query {
            id: format!("scale-needle-{}", node.id),
            text: phrase.join(" "),
            category: "name".to_string(),
            type_filter: None,
            note: Some("planted needle (issue #52: short concept vs. long document)".to_string()),
            relevant,
        });
    }
    for i in 0..N_LATENCY_QUERIES {
        let n_words = 1 + rng.gen_range(6);
        let text = (0..n_words)
            .map(|_| vocab.sample(&mut rng).to_string())
            .collect::<Vec<_>>()
            .join(" ");
        queries.push(Query {
            id: format!("scale-latency-{i:02}"),
            text,
            category: "latency".to_string(),
            type_filter: None,
            note: None,
            relevant: BTreeMap::new(),
        });
    }

    let notices = vec![format!(
        "scale corpus: {n_documents} document node(s), {} concept node(s) \
         ({n_needles} needle(s) + {n_filler_concepts} filler), seed={seed:#x}",
        n_needles + n_filler_concepts
    )];

    Ok((CorpusBuild::new(tempdir, notices), queries))
}

// `benches/search/main.rs` builds this file into a `harness = false` bench target, where
// `#[test]` items are compiled (cfg(test) is set for bench targets regardless of `harness`)
// but are not wired up as usage roots the way a real `--test` binary treats them — only
// `cargo test --test search_relevance` (which also includes this file) actually runs these.
// That harmless asymmetry makes rustc see some test-only helpers as unused in the bench
// build; allow it here rather than fight it.
#[cfg(test)]
#[allow(dead_code, unused_imports)]
mod tests {
    use super::*;

    #[test]
    fn strip_leading_frontmatter_removes_a_generic_yaml_block() {
        let src = "---\ntitle: X\nstatus: draft\n---\n\n# Heading\n\nBody.\n";
        assert_eq!(strip_leading_frontmatter(src), "\n# Heading\n\nBody.\n");
    }

    #[test]
    fn strip_leading_frontmatter_is_a_noop_without_a_fence() {
        let src = "# Heading\n\nBody.\n";
        assert_eq!(strip_leading_frontmatter(src), src);
    }

    #[test]
    fn strip_leading_frontmatter_is_a_noop_without_a_closing_fence() {
        let src = "---\ntitle: X\nno closing fence\n";
        assert_eq!(strip_leading_frontmatter(src), src);
    }

    #[test]
    fn rng_is_deterministic_for_a_fixed_seed() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let seq_a: Vec<u64> = (0..20).map(|_| a.next_u64()).collect();
        let seq_b: Vec<u64> = (0..20).map(|_| b.next_u64()).collect();
        assert_eq!(seq_a, seq_b);
    }

    #[test]
    fn gen_range_never_panics_on_zero() {
        let mut rng = Rng::new(1);
        assert_eq!(rng.gen_range(0), 0);
    }

    #[test]
    fn vocab_generates_the_requested_size_and_is_deterministic() {
        let mut r1 = Rng::new(SCALE_SEED);
        let v1 = Vocab::generate(&mut r1, 500);
        let mut r2 = Rng::new(SCALE_SEED);
        let v2 = Vocab::generate(&mut r2, 500);
        assert_eq!(v1.words.len(), 500);
        assert_eq!(v1.words, v2.words);
    }

    #[test]
    fn build_scale_is_deterministic_across_two_runs() {
        let (b1, q1) = build_scale(120, SCALE_SEED).unwrap();
        let (b2, q2) = build_scale(120, SCALE_SEED).unwrap();
        let f1 = walk_files(&b1.root).unwrap();
        let f2 = walk_files(&b2.root).unwrap();
        assert_eq!(f1.len(), f2.len());
        for (p1, p2) in f1.iter().zip(&f2) {
            let rel1 = p1.strip_prefix(&b1.root).unwrap();
            let rel2 = p2.strip_prefix(&b2.root).unwrap();
            assert_eq!(rel1, rel2);
            assert_eq!(
                std::fs::read_to_string(p1).unwrap(),
                std::fs::read_to_string(p2).unwrap()
            );
        }
        assert_eq!(q1.len(), q2.len());
        assert_eq!(q1[0].text, q2[0].text);
        // 30 needle (`name`) queries + 30 unjudged `latency` queries.
        assert_eq!(q1.iter().filter(|q| q.is_judged()).count(), N_NEEDLES);
        assert_eq!(q1.len(), N_NEEDLES + N_LATENCY_QUERIES);
    }

    #[test]
    fn build_scale_plants_every_needle_as_a_real_file() {
        let (build, queries) = build_scale(100, SCALE_SEED).unwrap();
        for q in queries.iter().filter(|q| q.is_judged()) {
            let id = q.primary_expected_id().unwrap();
            let (_, slug) = id.split_once(':').unwrap();
            let path = build
                .root
                .join(format!("generated/scale/needles/{slug}.md"));
            assert!(path.is_file(), "expected {} to exist", path.display());
        }
    }

    fn write_file(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn packing_the_md_tree_is_deterministic_and_round_trips() {
        let src = tempfile::tempdir().unwrap();
        write_file(src.path(), "concept/b.md", "---\nid: b\n---\nB\n");
        write_file(src.path(), "concept/a.md", "---\nid: a\n---\nA\n");
        write_file(src.path(), "decision/nested/c.md", "C\n");
        write_file(src.path(), "notes.txt", "not a node\n");
        write_file(src.path(), "concept/._a.md", "AppleDouble sidecar\n");

        let out = tempfile::tempdir().unwrap();
        let first = out.path().join("first.tar.gz");
        let second = out.path().join("second.tar.gz");
        assert_eq!(pack_md_tree(src.path(), &first).unwrap(), 3);
        assert_eq!(pack_md_tree(src.path(), &second).unwrap(), 3);
        assert_eq!(
            std::fs::read(&first).unwrap(),
            std::fs::read(&second).unwrap(),
            "the same files must pack to the same bytes"
        );

        let dest = tempfile::tempdir().unwrap();
        assert_eq!(unpack_md_archive(&first, dest.path()).unwrap(), 3);
        assert_eq!(
            std::fs::read_to_string(dest.path().join("concept/a.md")).unwrap(),
            "---\nid: a\n---\nA\n"
        );
        assert_eq!(
            std::fs::read_to_string(dest.path().join("decision/nested/c.md")).unwrap(),
            "C\n"
        );
        assert!(!dest.path().join("notes.txt").exists());
        assert!(!dest.path().join("concept/._a.md").exists());
    }

    #[test]
    fn packing_refuses_a_tree_without_markdown() {
        let src = tempfile::tempdir().unwrap();
        write_file(src.path(), "notes.txt", "not a node\n");
        let out = tempfile::tempdir().unwrap();
        assert!(pack_md_tree(src.path(), &out.path().join("empty.tar.gz")).is_err());
    }

    #[test]
    fn unpacking_refuses_an_entry_that_climbs_out() {
        let out = tempfile::tempdir().unwrap();
        let archive = out.path().join("climbs.tar.gz");
        let gz = flate2::GzBuilder::new().write(
            std::fs::File::create(&archive).unwrap(),
            flate2::Compression::new(6),
        );
        let mut tar = tar::Builder::new(gz);
        let body = b"escaped\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        // `append_data` itself refuses `..`, so write the name into the header directly.
        header.as_gnu_mut().unwrap().name[..11].copy_from_slice(b"../evil.md\0");
        header.set_cksum();
        tar.append(&header, &body[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();

        let dest = out.path().join("dest");
        assert!(unpack_md_archive(&archive, &dest).is_err());
        assert!(!out.path().join("evil.md").exists());
    }
}
