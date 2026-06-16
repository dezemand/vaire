//! Output discipline — the returned unit is **paths + IDs**, never file bodies
//! (cli.md §2.3, §2.4).
//!
//! Every command produces a value implementing [`Output`]: `--json` serializes it to
//! the exact shape MCP returns (so there is no second serialization to drift), and the
//! default path renders deterministic human text. stdout carries only the result;
//! progress/warnings/errors go to stderr.
//!
//! The structs below are the **canonical JSON shapes** from cli.md §3–§4. The `commands`
//! modules build them from the in-crate query/search results.

use serde::Serialize;

use crate::index::build::IndexSummary;
use crate::index::check::CheckReport;

pub mod style;
use style::{bold, cyan, dim, green, red, yellow};

/// Initialize human-output coloring from the `--no-color` flag (also honors `NO_COLOR`
/// and a non-tty stdout). Call once in `main` before rendering.
pub fn init_color(no_color_flag: bool) {
    style::init(style::auto(no_color_flag));
}

/// A command result that can render as human text or as its canonical JSON value.
///
/// `render_human` is the bespoke human layout (cli.md §3–§4); `to_json` is the canonical
/// machine shape that MCP returns verbatim. The default `render_human` pretty-prints the
/// JSON — every concrete type below overrides it.
pub trait Output: Serialize {
    /// Human-readable rendering (stable + deterministic).
    fn render_human(&self) -> String {
        serde_json::to_string_pretty(&self.to_json()).expect("output serializes")
    }

    /// The canonical JSON value (defaults to `serde_json` of `self`).
    fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("output serializes")
    }
}

// ---- human-rendering helpers ----------------------------------------------

/// An aligned `  label:    value` line (label dimmed, padded to `width`).
fn kv(out: &mut String, label: &str, width: usize, value: &str) {
    let label = format!("{:<width$}", format!("{label}:"));
    out.push_str(&format!("  {} {}\n", dim(&label), value));
}

/// `path:line`, dimmed — the clickable pointer the caller opens.
fn loc(path: &str, line: u32) -> String {
    dim(&format!("{path}:{line}"))
}

/// Render one scalar/array JSON frontmatter value as a single line.
fn json_inline(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a.iter().map(json_inline).collect::<Vec<_>>().join(", "),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Max display width of `ids`, for column alignment.
fn col_width<'a>(ids: impl Iterator<Item = &'a str>) -> usize {
    ids.map(str::len).max().unwrap_or(0)
}

fn pluralize(n: usize, singular: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {singular}s")
    }
}

impl Output for ResolveOutput {
    fn render_human(&self) -> String {
        let mut out = String::new();
        out.push_str(&bold(&cyan(&self.id)));
        out.push('\n');
        if let (Some(req), Some(_target)) = (&self.requested_id, &self.superseded_by) {
            out.push_str(&dim(&format!("  ↳ superseded; requested {req}\n")));
        }
        kv(&mut out, "path", 8, &self.path);
        kv(&mut out, "type", 8, &self.node_type);
        if let Some(obj) = self.frontmatter.as_object() {
            // `name` is the display name — show it first; then the rest, alphabetical.
            if let Some(name) = obj.get("name") {
                kv(&mut out, "name", 8, &json_inline(name));
            }
            for (key, value) in obj {
                if key != "name" {
                    kv(&mut out, key, 8, &json_inline(value));
                }
            }
        }
        out.trim_end().to_string()
    }
}

impl Output for BacklinksOutput {
    fn render_human(&self) -> String {
        if self.backlinks.is_empty() {
            return dim(&format!("no nodes reference {}", self.id));
        }
        let mut out = format!(
            "{} reference {}\n",
            pluralize(self.count, "node"),
            bold(&cyan(&self.id))
        );
        let w = col_width(self.backlinks.iter().map(|b| b.id.as_str()));
        for b in &self.backlinks {
            out.push_str(&format!(
                "  {}  {}  {}\n",
                cyan(&format!("{:<w$}", b.id)),
                dim(&format!("{:<12}", b.ref_type)),
                loc(&b.path, b.line),
            ));
        }
        out.trim_end().to_string()
    }
}

impl Output for RefsOutput {
    fn render_human(&self) -> String {
        if self.refs.is_empty() {
            return dim(&format!("{} references nothing", self.id));
        }
        let mut out = format!(
            "{} → {} (depth {})\n",
            bold(&cyan(&self.id)),
            pluralize(self.count, "node"),
            self.depth,
        );
        let w = col_width(self.refs.iter().map(|r| r.id.as_str()));
        let show_dist = self.depth > 1;
        for r in &self.refs {
            let prefix = if show_dist {
                dim(&format!("[{}] ", r.distance.unwrap_or(1)))
            } else {
                String::new()
            };
            out.push_str(&format!(
                "  {}{}  {}  {}\n",
                prefix,
                cyan(&format!("{:<w$}", r.id)),
                dim(&format!("{:<12}", r.ref_type)),
                loc(&r.path, r.line),
            ));
        }
        out.trim_end().to_string()
    }
}

impl Output for SearchOutput {
    fn render_human(&self) -> String {
        if self.results.is_empty() {
            return dim(&format!("no results for \"{}\"", self.query));
        }
        let mut out = format!(
            "{} for {}\n",
            pluralize(self.count, "result"),
            bold(&format!("\"{}\"", self.query))
        );
        for r in &self.results {
            out.push_str(&format!(
                "  {}  {}  {}\n",
                dim(&format!("{:.2}", r.score)),
                cyan(&r.id),
                dim(&r.path),
            ));
            for a in &r.anchors {
                out.push_str(&format!(
                    "      {}  {}\n",
                    dim(&format!("{}:{}", a.heading, a.line)),
                    a.snippet,
                ));
            }
        }
        out.trim_end().to_string()
    }
}

impl Output for UnresolvedOutput {
    fn render_human(&self) -> String {
        if self.unresolved.is_empty() {
            return dim("no unresolved references");
        }
        let mut out = format!("{}\n", pluralize(self.count, "unresolved reference"));
        let tags: Vec<String> = self
            .unresolved
            .iter()
            .map(|u| match &u.type_guess {
                Some(t) => format!("?{t}"),
                None => "?".to_string(),
            })
            .collect();
        let w = col_width(tags.iter().map(String::as_str));
        let descs: Vec<String> = self
            .unresolved
            .iter()
            .map(|u| format!("\"{}\"", u.descriptor))
            .collect();
        let dw = col_width(descs.iter().map(String::as_str));
        for ((u, tag), desc) in self.unresolved.iter().zip(&tags).zip(&descs) {
            out.push_str(&format!(
                "  {}  {desc:<dw$}  {}  {}\n",
                yellow(&format!("{:<w$}", tag)),
                loc(&u.path, u.line),
                dim(&format!("({})", u.record)),
            ));
        }
        out.trim_end().to_string()
    }
}

impl Output for StatusOutput {
    fn render_human(&self) -> String {
        let mut out = String::new();
        kv(&mut out, "repo", 13, &self.repo);
        kv(&mut out, "index", 13, &self.index_path);
        if let Some(v) = self.schema_version {
            kv(&mut out, "schema", 13, &v.to_string());
        }

        let last = if let Some(c) = &self.last_indexed_commit {
            let short = &c[..c.len().min(7)];
            let suffix = if self.commits_behind_head == 0 {
                dim("(up to date)")
            } else {
                yellow(&format!(
                    "({} commits behind HEAD)",
                    self.commits_behind_head
                ))
            };
            format!("{short}  {suffix}")
        } else if self.source.as_deref() == Some("working-tree") {
            // Built, but from uncommitted edits — not "not built yet".
            yellow("working tree (uncommitted — not a commit)")
        } else {
            dim("not built yet")
        };
        kv(&mut out, "last-indexed", 13, &last);

        let by_type = self
            .nodes
            .by_type
            .iter()
            .map(|(t, n)| format!("{t} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let nodes = if by_type.is_empty() {
            self.nodes.total.to_string()
        } else {
            format!("{}   {}", self.nodes.total, dim(&format!("({by_type})")))
        };
        kv(&mut out, "nodes", 13, &nodes);
        kv(&mut out, "edges", 13, &self.edges.to_string());
        kv(
            &mut out,
            "embeddings",
            13,
            &format!(
                "cached {} / {} sections",
                self.embeddings.cached, self.embeddings.sections
            ),
        );
        out.trim_end().to_string()
    }
}

impl Output for IndexSummary {
    fn render_human(&self) -> String {
        let commit = match &self.commit {
            Some(c) => format!("commit {}", &c[..c.len().min(7)]),
            None => "working tree".to_string(),
        };
        format!(
            "indexed {} · {} · {} in {}ms  {}",
            pluralize(self.nodes, "node"),
            pluralize(self.edges, "edge"),
            pluralize(self.sections_embedded, "section"),
            self.elapsed_ms,
            dim(&format!("({commit})")),
        )
    }
}

impl Output for CheckReport {
    fn render_human(&self) -> String {
        if self.violations.is_empty() && self.warnings.is_empty() {
            return green("✓ no violations");
        }
        let mut out = String::new();
        let head = format!(
            "{}, {}",
            pluralize(self.violations.len(), "violation"),
            pluralize(self.warnings.len(), "warning")
        );
        if self.violations.is_empty() {
            out.push_str(&format!("{} {head}\n", green("✓")));
        } else {
            out.push_str(&format!("{} {head}\n", red("✗")));
        }
        for v in &self.violations {
            out.push_str(&format!(
                "  {}  {}\n",
                red(&format!("{:<13}", v.kind())),
                v.detail()
            ));
        }
        for w in &self.warnings {
            out.push_str(&format!(
                "  {}  {}\n",
                yellow(&format!("{:<13}", w.kind())),
                w.detail()
            ));
        }
        out.trim_end().to_string()
    }
}

// ---- init ------------------------------------------------------------------

/// `vaire init`: the corpus that was scaffolded.
#[derive(Debug, Serialize)]
pub struct InitOutput {
    pub root: String,
    pub config_path: String,
}

impl Output for InitOutput {
    fn render_human(&self) -> String {
        format!(
            "{} initialized Vairë corpus\n  root:   {}\n  config: {}\n  next:   vaire index",
            green("✓"),
            self.root,
            self.config_path,
        )
    }
}

// ---- render (rendered Markdown) --------------------------------------------

/// `vaire render <id>`: the node's Markdown with frontmatter kept and wikilinks
/// resolved to `[name](relative-path)`. Unlike the pointer-returning read commands,
/// this returns a **body** — the human form is the Markdown itself.
#[derive(Debug, Serialize)]
pub struct RenderOutput {
    pub id: String,
    pub path: String,
    pub markdown: String,
}

impl Output for RenderOutput {
    fn render_human(&self) -> String {
        self.markdown.clone()
    }
}

// ---- resolve (cli.md §3.1) -------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ResolveOutput {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub path: String,
    pub frontmatter: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_id: Option<String>,
    pub superseded_by: Option<String>,
}

// ---- backlinks (cli.md §3.2) -----------------------------------------------

#[derive(Debug, Serialize)]
pub struct BacklinksOutput {
    pub id: String,
    pub backlinks: Vec<EdgeRef>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct EdgeRef {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub path: String,
    pub ref_type: String,
    pub line: u32,
    /// Present on `refs` output (distance from the query node); omitted on backlinks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<u32>,
}

// ---- refs (cli.md §3.3) ----------------------------------------------------

#[derive(Debug, Serialize)]
pub struct RefsOutput {
    pub id: String,
    pub depth: u32,
    pub refs: Vec<EdgeRef>,
    pub count: usize,
}

// ---- search (cli.md §3.4) --------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SearchOutput {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub path: String,
    pub score: f32,
    pub anchors: Vec<AnchorOut>,
}

#[derive(Debug, Serialize)]
pub struct AnchorOut {
    pub heading: String,
    pub line: u32,
    pub snippet: String,
}

// ---- suggest (cli.md §3.7) -------------------------------------------------

/// `vaire suggest <descriptor>`: ranked existing IDs a descriptor might refer to.
#[derive(Debug, Serialize)]
pub struct SuggestOutput {
    pub descriptor: String,
    pub suggestions: Vec<SuggestionItem>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct SuggestionItem {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub name: String,
    pub path: String,
    pub score: f32,
}

impl Output for SuggestOutput {
    fn render_human(&self) -> String {
        if self.suggestions.is_empty() {
            return dim(&format!("no suggestions for \"{}\"", self.descriptor));
        }
        let mut out = format!(
            "{} for {}\n",
            pluralize(self.count, "suggestion"),
            bold(&format!("\"{}\"", self.descriptor))
        );
        let w = col_width(self.suggestions.iter().map(|s| s.id.as_str()));
        for s in &self.suggestions {
            out.push_str(&format!(
                "  {}  {}  {}  {}\n",
                dim(&format!("{:.2}", s.score)),
                cyan(&format!("{:<w$}", s.id)),
                s.name,
                dim(&s.path),
            ));
        }
        out.trim_end().to_string()
    }
}

// ---- unresolved (cli.md §3.5) ----------------------------------------------

#[derive(Debug, Serialize)]
pub struct UnresolvedOutput {
    pub unresolved: Vec<UnresolvedItem>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct UnresolvedItem {
    pub record: String,
    pub path: String,
    pub type_guess: Option<String>,
    pub descriptor: String,
    pub line: u32,
}

// ---- status (cli.md §4.3) --------------------------------------------------

#[derive(Debug, Serialize)]
pub struct StatusOutput {
    pub repo: String,
    pub index_path: String,
    /// The index's schema version, or `null` when not built.
    pub schema_version: Option<u32>,
    /// What the index reflects: `"committed"`, `"working-tree"`, or `null` (not built).
    pub source: Option<String>,
    pub last_indexed_commit: Option<String>,
    pub commits_behind_head: u32,
    pub nodes: NodeCounts,
    pub edges: usize,
    pub embeddings: EmbeddingCounts,
}

#[derive(Debug, Serialize)]
pub struct NodeCounts {
    pub total: usize,
    pub by_type: std::collections::BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
pub struct EmbeddingCounts {
    pub sections: usize,
    pub cached: usize,
}
