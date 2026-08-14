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
use style::{bold, cyan, dim, green, inline_text, plain, red, yellow};

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
    let label = format!("{:<width$}", format!("{}:", plain(label)));
    out.push_str(&format!("  {} {}\n", dim(&label), plain(value)));
}

/// `path:line`, dimmed — the clickable pointer the caller opens.
fn loc(path: &str, line: u32) -> String {
    dim(&format!("{}:{line}", plain(path)))
}

/// Render one scalar/array JSON frontmatter value as a single line.
fn json_inline(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => inline_text(s),
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
        kv(
            &mut out,
            "path",
            8,
            self.display_path.as_deref().unwrap_or(&self.path),
        );
        kv(&mut out, "type", 8, &self.node_type);
        if let Some(package) = &self.package {
            kv(&mut out, "package", 8, package);
        }
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
            return format!(
                "{}{}",
                dim(&format!("no nodes reference {}", self.id)),
                skipped_note(&self.skipped)
            );
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
                loc(b.human_path(), b.line),
            ));
        }
        let mut out = out.trim_end().to_string();
        out.push_str(&skipped_note(&self.skipped));
        out
    }
}

impl Output for RefsOutput {
    fn render_human(&self) -> String {
        if self.refs.is_empty() {
            return format!(
                "{}{}",
                dim(&format!("{} references nothing", self.id)),
                skipped_note(&self.skipped)
            );
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
                loc(r.human_path(), r.line),
            ));
        }
        let mut out = out.trim_end().to_string();
        out.push_str(&skipped_note(&self.skipped));
        out
    }
}

impl Output for SearchOutput {
    fn render_human(&self) -> String {
        if self.results.is_empty() {
            return format!(
                "{}{}",
                dim(&format!("no results for \"{}\"", self.query)),
                skipped_note(&self.skipped)
            );
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
                dim(r.display_path.as_deref().unwrap_or(&r.path)),
            ));
            for a in &r.anchors {
                out.push_str(&format!(
                    "      {}  {}\n",
                    dim(&format!("{}:{}", a.heading, a.line)),
                    plain(&a.snippet),
                ));
            }
        }
        let mut out = out.trim_end().to_string();
        out.push_str(&skipped_note(&self.skipped));
        out
    }
}

impl Output for UnresolvedOutput {
    fn render_human(&self) -> String {
        if self.unresolved.is_empty() {
            return format!(
                "{}{}",
                dim("no unresolved references"),
                skipped_note(&self.skipped)
            );
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
            .map(|u| format!("\"{}\"", plain(&u.descriptor)))
            .collect();
        let dw = col_width(descs.iter().map(String::as_str));
        for ((u, tag), desc) in self.unresolved.iter().zip(&tags).zip(&descs) {
            out.push_str(&format!(
                "  {}  {desc:<dw$}  {}  {}\n",
                yellow(&format!("{:<w$}", tag)),
                loc(u.display_path.as_deref().unwrap_or(&u.path), u.line),
                dim(&format!("({})", u.record)),
            ));
        }
        let mut out = out.trim_end().to_string();
        out.push_str(&skipped_note(&self.skipped));
        out
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
        if !self.dependencies.is_empty() {
            out.push_str("dependencies:\n");
            let w = col_width(self.dependencies.iter().map(|d| d.name.as_str()));
            for d in &self.dependencies {
                if !d.linked || d.index == "missing" || d.index == "unreadable" {
                    // Unavailable in some way: name + state + the note/fix.
                    let detail = d.note.as_deref().unwrap_or("no index — run `vaire index`");
                    out.push_str(&format!(
                        "  {}  {}\n",
                        cyan(&format!("{:<w$}", d.name)),
                        yellow(&format!("{} — {detail}", d.index)),
                    ));
                    continue;
                }
                let commit = match &d.last_indexed_commit {
                    Some(c) => {
                        let short = &c[..c.len().min(7)];
                        if d.commits_behind_head == 0 {
                            format!("{short} {}", dim("(up to date)"))
                        } else {
                            format!(
                                "{short} {}",
                                yellow(&format!("({} behind)", d.commits_behind_head))
                            )
                        }
                    }
                    None => dim("working tree").to_string(),
                };
                let mut line = format!(
                    "  {}  {}  {}  {}",
                    cyan(&format!("{:<w$}", d.name)),
                    d.index,
                    pluralize(d.nodes, "node"),
                    commit,
                );
                // The observability point for silent vector-recall gaps: a dep embedded
                // by a different provider contributes FTS/alias hits only.
                if let (Some(theirs), Some(ours)) = (&d.embed_provider, &self.embed_provider)
                    && theirs != ours
                {
                    line.push_str(&format!(
                        "  {}",
                        yellow(&format!(
                            "vectors {theirs} vs yours {ours} — vector search skipped"
                        ))
                    ));
                }
                out.push_str(&line);
                out.push('\n');
            }
        }
        if let Some(pending) = &self.pending_release {
            kv(&mut out, "release", 13, &pending.render());
        }
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

/// One linked dependency's outcome during `vaire index`'s ensure pass (cli.md §6.5).
#[derive(Debug, Serialize)]
pub struct DepIndexed {
    pub name: String,
    /// `"indexed"` (built or refreshed, possibly a no-op) or `"missing"` (not linked /
    /// broken link / name mismatch — see `note`; tolerated with a warning, `vaire check`
    /// escalates).
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The package directory this run linked the dependency to, when the ensure pass
    /// satisfied it from the catalog (cli.md §6.6); absent when the link already existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked: Option<String>,
}

/// `vaire index`: the current package's summary plus the linked-dependency ensure pass.
#[derive(Debug, Serialize)]
pub struct IndexRunOutput {
    #[serde(flatten)]
    pub summary: IndexSummary,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<DepIndexed>,
}

impl Output for IndexRunOutput {
    fn render_human(&self) -> String {
        let mut out = self.summary.render_human();
        for dep in &self.dependencies {
            out.push('\n');
            match dep.status.as_str() {
                "indexed" => {
                    let commit = match &dep.commit {
                        Some(c) => format!("commit {}", &c[..c.len().min(7)]),
                        None => "working tree".to_string(),
                    };
                    // A link this run created is worth saying out loud: it is the moment
                    // a declared dependency became an actual one.
                    if let Some(target) = &dep.linked {
                        out.push_str(&format!(
                            "  dep {}: linked → {}  {}\n",
                            dep.name,
                            target,
                            dim("(catalog)")
                        ));
                    }
                    out.push_str(&format!(
                        "  dep {}: indexed {}  {}",
                        dep.name,
                        pluralize(dep.nodes.unwrap_or(0), "node"),
                        dim(&format!("({commit})")),
                    ));
                }
                _ => {
                    out.push_str(&yellow(&format!(
                        "  dep {}: {} — {}",
                        dep.name,
                        dep.status,
                        dep.note.as_deref().unwrap_or("unavailable"),
                    )));
                }
            }
        }
        out
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
                plain(&v.detail())
            ));
        }
        for w in &self.warnings {
            out.push_str(&format!(
                "  {}  {}\n",
                yellow(&format!("{:<13}", w.kind())),
                plain(&w.detail())
            ));
        }
        out.trim_end().to_string()
    }
}

// ---- init ------------------------------------------------------------------

/// `vaire init`: the package that was scaffolded or migrated.
#[derive(Debug, Serialize)]
pub struct InitOutput {
    pub root: String,
    pub config_path: String,
    /// True when an existing `.vaire/config.toml` was migrated into `knowledge.toml`.
    pub migrated: bool,
}

impl Output for InitOutput {
    fn render_human(&self) -> String {
        let verb = if self.migrated {
            "migrated to knowledge.toml"
        } else {
            "initialized Vairë package"
        };
        format!(
            "{} {}\n  root:     {}\n  manifest: {}\n  next:     vaire index",
            green("✓"),
            verb,
            self.root,
            self.config_path,
        )
    }
}

/// `vaire add`: the dependency written to `[dependencies]` in the manifest.
#[derive(Debug, Serialize)]
pub struct AddOutput {
    pub name: String,
    pub constraint: String,
    pub config_path: String,
    /// True when the dependency already existed and its constraint was updated in place.
    pub updated: bool,
    /// The `.vaire/packages/<name>` link target: the path stored by `--link`, or the
    /// package directory the catalog selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked: Option<String>,
    /// The link was materialized from the catalog (cli.md §6.6) rather than by `--link`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub discovered: bool,
    /// Why the dependency is declared but not linked — the catalog knows nothing
    /// declaring that name, nothing in the constrained major line, or two candidates that
    /// both satisfy. Never an error: declaring succeeded, and the link can arrive later.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Output for AddOutput {
    fn render_human(&self) -> String {
        let verb = if self.updated { "updated" } else { "added" };
        let mut s = format!(
            "{} {verb} dependency\n  {} = \"{}\"\n  manifest: {}",
            green("✓"),
            self.name,
            self.constraint,
            self.config_path,
        );
        if let Some(target) = &self.linked {
            s.push_str(&format!(
                "\n  linked:   .vaire/packages/{} → {}",
                self.name, target
            ));
            if self.discovered {
                s.push_str(&format!("  {}", dim("(catalog)")));
            }
        }
        if let Some(note) = &self.note {
            s.push_str(&format!("\n  {}", yellow(&format!("not linked — {note}"))));
        }
        s
    }
}

/// `vaire configure`: the embedding settings written to the global user config.
#[derive(Debug, Serialize)]
pub struct ConfigureOutput {
    pub config_path: String,
    /// Which section this run reports on. Only `"embeddings"` remains — where packages
    /// live stopped being a setting when the catalog replaced the local-packages root.
    pub section: String,
    pub provider: String,
    pub dimensions: usize,
    /// Secret keys written to `credentials.toml` this run (values never shown).
    pub credentials_set: Vec<String>,
    /// The interactive flow was cancelled (Esc/Ctrl-C); nothing was written.
    #[serde(default)]
    pub cancelled: bool,
}

impl Output for ConfigureOutput {
    fn render_human(&self) -> String {
        if self.cancelled {
            return "Cancelled — no changes written.".to_string();
        }
        let mut s = format!(
            "{} configured embeddings\n  provider:   {}\n  dimensions: {}\n  config:     {}",
            green("✓"),
            self.provider,
            self.dimensions,
            self.config_path,
        );
        if !self.credentials_set.is_empty() {
            s.push_str(&format!(
                "\n  secrets:    {} (credentials.toml)",
                self.credentials_set.join(", ")
            ));
        }
        s
    }
}

// ---- upgrade (cli.md §4.5) --------------------------------------------------

/// `vaire upgrade`: what release was resolved and what happened to the binary.
#[derive(Debug, Serialize)]
pub struct UpgradeOutput {
    /// The running version (bare, e.g. `0.2.0`).
    pub current: String,
    /// The release version resolved or pinned (bare, e.g. `0.3.0` — the `v` prefix
    /// exists only on the underlying git tag).
    pub latest: String,
    /// The release target triple this binary matches.
    pub target: String,
    pub up_to_date: bool,
    /// `--check`: nothing was downloaded or written.
    pub checked_only: bool,
    /// Path the new binary was installed to (absent when nothing was installed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Output for UpgradeOutput {
    fn render_human(&self) -> String {
        let mut s = if let Some(path) = &self.installed {
            format!(
                "{} upgraded vaire {} → {} {}\n  installed: {}",
                green("✓"),
                self.current,
                self.latest,
                dim(&format!("({})", self.target)),
                path,
            )
        } else if self.up_to_date {
            format!(
                "vaire {} is up to date {}",
                self.current,
                dim(&format!("(latest release: {})", self.latest)),
            )
        } else {
            format!(
                "vaire {} → {} available. Run `vaire upgrade` to install.",
                self.current,
                bold(&self.latest),
            )
        };
        if let Some(note) = &self.note {
            s.push_str(&format!("\n  {}", dim(&plain(note))));
        }
        s
    }
}

// ---- render (rendered Markdown) --------------------------------------------

/// `vaire render <id>`: the node's Markdown with frontmatter kept and wikilinks
/// resolved to `[name](relative-path)`. Unlike the pointer-returning read commands,
/// this returns a **body** — the human form is the Markdown itself.
#[derive(Debug, Serialize)]
pub struct RenderOutput {
    pub id: String,
    /// Package-root-relative (see [`ResolveOutput::path`]).
    pub path: String,
    /// The owning package for a cross-package node; absent/null = the current package.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    pub markdown: String,
}

impl Output for RenderOutput {
    fn render_human(&self) -> String {
        plain(&self.markdown)
    }
}

// ---- resolve (cli.md §3.1) -------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ResolveOutput {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    /// Package-root-relative — stable across workspace and future cache layouts.
    pub path: String,
    /// The owning package for a cross-package node; absent/null = the current package.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    pub frontmatter: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_id: Option<String>,
    pub superseded_by: Option<String>,
    /// Human display only: the clickable consumer-relative path for a cross-package
    /// node (`../acme-core/…`), computed live — never serialized.
    #[serde(skip)]
    pub display_path: Option<String>,
}

// ---- backlinks (cli.md §3.2) -----------------------------------------------

#[derive(Debug, Serialize)]
pub struct BacklinksOutput {
    pub id: String,
    pub backlinks: Vec<EdgeRef>,
    pub count: usize,
    /// Dependencies that could not be consulted (unlinked / broken / no index) —
    /// surfaced, never silently dropped.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct EdgeRef {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    /// Package-root-relative — stable across workspace and future cache layouts.
    pub path: String,
    /// The owning package for a cross-package row; absent/null = the current package.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    pub ref_type: String,
    pub line: u32,
    /// Present on `refs` output (distance from the query node); omitted on backlinks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<u32>,
    /// Human display only: clickable consumer-relative path for a cross-package row.
    #[serde(skip)]
    pub display_path: Option<String>,
}

impl EdgeRef {
    /// The path as shown to a human: clickable consumer-relative when cross-package.
    fn human_path(&self) -> &str {
        self.display_path.as_deref().unwrap_or(&self.path)
    }
}

/// The shared "skipped dependencies" trailer for fan-out reads.
fn skipped_note(skipped: &[String]) -> String {
    if skipped.is_empty() {
        String::new()
    } else {
        format!(
            "\n{}",
            yellow(&format!(
                "  note: skipped unavailable dependencies: {} (run `vaire index`)",
                skipped.join(", ")
            ))
        )
    }
}

// ---- refs (cli.md §3.3) ----------------------------------------------------

#[derive(Debug, Serialize)]
pub struct RefsOutput {
    pub id: String,
    pub depth: u32,
    pub refs: Vec<EdgeRef>,
    pub count: usize,
    /// Dependencies that could not be consulted — surfaced, never silently dropped.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

// ---- search (cli.md §3.4) --------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SearchOutput {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub count: usize,
    /// Dependencies that could not be consulted — surfaced, never silently dropped.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    /// Package-root-relative — stable across workspace and future cache layouts.
    pub path: String,
    /// The owning package for a cross-package hit; absent/null = the current package.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    pub score: f32,
    pub anchors: Vec<AnchorOut>,
    /// Human display only: clickable consumer-relative path for a cross-package hit.
    #[serde(skip)]
    pub display_path: Option<String>,
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
    /// Dependencies that could not be consulted — surfaced, never silently dropped.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SuggestionItem {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub name: String,
    /// Package-root-relative — stable across workspace and future cache layouts.
    pub path: String,
    /// The owning package for a cross-package suggestion; absent/null = local.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    pub score: f32,
    /// Human display only: clickable consumer-relative path for a cross-package hit.
    #[serde(skip)]
    pub display_path: Option<String>,
}

impl Output for SuggestOutput {
    fn render_human(&self) -> String {
        if self.suggestions.is_empty() {
            return format!(
                "{}{}",
                dim(&format!("no suggestions for \"{}\"", self.descriptor)),
                skipped_note(&self.skipped)
            );
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
                plain(&s.name),
                dim(s.display_path.as_deref().unwrap_or(&s.path)),
            ));
        }
        let mut out = out.trim_end().to_string();
        out.push_str(&skipped_note(&self.skipped));
        out
    }
}

// ---- unresolved (cli.md §3.5) ----------------------------------------------

#[derive(Debug, Serialize)]
pub struct UnresolvedOutput {
    pub unresolved: Vec<UnresolvedItem>,
    pub count: usize,
    /// Dependencies that could not be consulted (`--all-packages` only) — surfaced,
    /// never silently dropped.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct UnresolvedItem {
    pub record: String,
    /// Package-root-relative — stable across workspace and future cache layouts.
    pub path: String,
    /// The owning package (`--all-packages` rows); absent/null = the current package.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    pub type_guess: Option<String>,
    pub descriptor: String,
    pub line: u32,
    /// Human display only: clickable consumer-relative path for a cross-package row.
    #[serde(skip)]
    pub display_path: Option<String>,
}

// ---- deps (cli.md §3.8) ----------------------------------------------------

/// `vaire deps`: the resolved local dependency tree (live link inspection, no index).
#[derive(Debug, Serialize)]
pub struct DepsOutput {
    pub name: String,
    pub version: String,
    pub dependencies: Vec<DepNode>,
}

#[derive(Debug, Serialize)]
pub struct DepNode {
    pub name: String,
    pub constraint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where the link resolves, relative to the run-root package (null = unavailable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<String>,
    /// Whether the resolved version's MAJOR satisfies the constraint (surfaced only —
    /// enforcement is v0.3); null when unresolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub satisfied: Option<bool>,
    /// This dependency closes a cycle back to a package already on this path; its own
    /// subtree is not repeated.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cycle: bool,
    /// Why the dependency is unavailable, when it is (with the fix).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<DepNode>,
}

impl Output for DepsOutput {
    fn render_human(&self) -> String {
        let mut out = format!("{} {}\n", bold(&cyan(&self.name)), dim(&self.version));
        render_dep_nodes(&mut out, &self.dependencies, "");
        out.trim_end().to_string()
    }
}

fn render_dep_nodes(out: &mut String, nodes: &[DepNode], prefix: &str) {
    for (i, node) in nodes.iter().enumerate() {
        let last = i == nodes.len() - 1;
        let branch = if last { "└── " } else { "├── " };
        let line = if let Some(note) = &node.note {
            format!(
                "{}{} {}  {}",
                cyan(&node.name),
                dim(&node.constraint),
                yellow("MISSING"),
                yellow(note),
            )
        } else {
            let version = node.version.as_deref().unwrap_or("?");
            let marker = match node.satisfied {
                Some(false) => format!(
                    "  {}",
                    yellow(&format!("({version} outside {})", node.constraint))
                ),
                _ => format!("  {}", dim(&format!("({version})"))),
            };
            let cycle = if node.cycle {
                format!("  {}", dim("(cycle)"))
            } else {
                String::new()
            };
            format!(
                "{} {} → {}{marker}{cycle}",
                cyan(&node.name),
                dim(&node.constraint),
                node.resolved.as_deref().unwrap_or("?"),
            )
        };
        out.push_str(&format!("{prefix}{branch}{line}\n"));
        let child_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
        render_dep_nodes(out, &node.dependencies, &child_prefix);
    }
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
    /// Which embedder produced this index's vectors (`provider[:model]:dims`), or null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed_provider: Option<String>,
    /// One row per linked dependency (transitive closure); empty for a standalone package.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<DepStatus>,
    /// What `vaire release` would do — absent when it could not be determined cheaply
    /// (no git, or an index behind HEAD, where the answer would describe neither tree).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_release: Option<PendingRelease>,
}

/// One linked dependency's state as reported by `vaire status` (cli.md §4.3).
#[derive(Debug, Serialize)]
pub struct DepStatus {
    pub name: String,
    pub linked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_indexed_commit: Option<String>,
    pub commits_behind_head: u32,
    pub nodes: usize,
    /// `"fresh"`, `"stale-schema"`, `"missing"`, or `"unreadable"`.
    pub index: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed_provider: Option<String>,
    /// Why the dependency is unavailable, when it is (with the fix).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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

// ---- pack ------------------------------------------------------------------

/// `vaire pack`: the artifact that was built (registry.md §5). `artifact` is
/// package-root-relative like every other returned path; `sha256` is the digest a
/// lockfile or registry pins.
#[derive(Debug, Serialize)]
pub struct PackOutput {
    pub name: String,
    pub version: String,
    /// The commit the artifact was packed from (the committed tree, always).
    pub commit: String,
    pub artifact: String,
    pub sha256: String,
    pub size_bytes: u64,
    /// Files in the archive (manifest + corpus files + referenced payload + the index).
    pub entries: usize,
    pub nodes: usize,
    /// Section vectors shipped in the artifact index (0 under `--no-embeddings`).
    pub embeddings: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl Output for PackOutput {
    fn render_human(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{} {} {}\n",
            green("packed"),
            bold(&cyan(&self.name)),
            plain(&self.version)
        ));
        kv(&mut out, "artifact", 10, &self.artifact);
        kv(&mut out, "sha256", 10, &self.sha256);
        kv(&mut out, "size", 10, &human_size(self.size_bytes));
        kv(
            &mut out,
            "contents",
            10,
            &format!(
                "{}, {} node{}, {} embedded section{}",
                pluralize(self.entries, "file"),
                self.nodes,
                if self.nodes == 1 { "" } else { "s" },
                self.embeddings,
                if self.embeddings == 1 { "" } else { "s" },
            ),
        );
        kv(
            &mut out,
            "commit",
            10,
            &self.commit[..self.commit.len().min(12)],
        );
        for warning in &self.warnings {
            out.push_str(&yellow(&format!("  warning: {warning}\n")));
        }
        out.trim_end().to_string()
    }
}

fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB ({bytes} bytes)", bytes as f64 / 1024.0)
    } else {
        format!(
            "{:.1} MiB ({bytes} bytes)",
            bytes as f64 / (1024.0 * 1024.0)
        )
    }
}

// ---- vaire release ---------------------------------------------------------

/// What a `vaire release` run did, or would do (cli.md §4.7).
#[derive(Debug, Serialize)]
pub struct ReleaseOutput {
    pub package: String,
    pub status: ReleaseStatus,
    /// The version released or planned; for `nothing`/`blocked`, the one already out.
    pub version: String,
    /// Absent for a first release (nothing to increment) and when nothing changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bump: Option<crate::model::Bump>,
    #[serde(flatten)]
    pub classification: crate::release::classify::Classification,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Integrity warnings the release carried past — reported, never fatal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<usize>,
    /// Changed entities a lot of others point at — the classifier saying "structure says
    /// patch, but this is load-bearing". Advisory; the maintainer's answer stands.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub advisories: Vec<CitedEntity>,
}

/// A changed entity with an inbound-reference count worth mentioning.
#[derive(Debug, Serialize)]
pub struct CitedEntity {
    pub id: String,
    pub inbound: usize,
}

/// The four ways a release run ends. `blocked` is the one that is neither success nor
/// failure: the classifier saw a MAJOR, which only a maintainer may declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseStatus {
    Released,
    Planned,
    Nothing,
    Blocked,
}

impl ReleaseOutput {
    fn base(
        package: String,
        status: ReleaseStatus,
        version: crate::model::Version,
        classification: crate::release::classify::Classification,
    ) -> ReleaseOutput {
        ReleaseOutput {
            package,
            status,
            version: version.to_string(),
            bump: None,
            classification,
            tag: None,
            record: None,
            commit: None,
            warnings: None,
            advisories: Vec::new(),
        }
    }

    /// Attach the heavily-cited advisories (chainable — every constructor may carry them).
    pub fn with_advisories(mut self, advisories: Vec<CitedEntity>) -> ReleaseOutput {
        self.advisories = advisories;
        self
    }

    pub fn nothing(
        package: String,
        version: crate::model::Version,
        classification: crate::release::classify::Classification,
    ) -> ReleaseOutput {
        ReleaseOutput::base(package, ReleaseStatus::Nothing, version, classification)
    }

    pub fn blocked(
        package: String,
        version: crate::model::Version,
        classification: crate::release::classify::Classification,
    ) -> ReleaseOutput {
        ReleaseOutput::base(package, ReleaseStatus::Blocked, version, classification)
    }

    pub fn planned(
        package: String,
        version: crate::model::Version,
        bump: Option<crate::model::Bump>,
        classification: crate::release::classify::Classification,
        tag: String,
        record: String,
    ) -> ReleaseOutput {
        ReleaseOutput {
            bump,
            tag: Some(tag),
            record: Some(record),
            ..ReleaseOutput::base(package, ReleaseStatus::Planned, version, classification)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn released(
        package: String,
        version: crate::model::Version,
        bump: Option<crate::model::Bump>,
        classification: crate::release::classify::Classification,
        tag: String,
        record: crate::release::record::Record,
        commit: String,
        warnings: usize,
    ) -> ReleaseOutput {
        ReleaseOutput {
            bump,
            tag: Some(tag),
            record: Some(record.path),
            commit: Some(commit),
            warnings: Some(warnings),
            ..ReleaseOutput::base(package, ReleaseStatus::Released, version, classification)
        }
    }
}

impl Output for ReleaseOutput {
    fn render_human(&self) -> String {
        let mut out = String::new();
        match self.status {
            ReleaseStatus::Nothing => {
                return format!(
                    "{} {}",
                    dim("nothing to release —"),
                    plain(&format!(
                        "{} {} is current, and no entity has changed since",
                        self.package, self.version
                    ))
                );
            }
            ReleaseStatus::Blocked => {
                out.push_str(&yellow(&format!(
                    "✗ this would be a MAJOR release of {} ({})\n",
                    self.package,
                    self.classification.evidence()
                )));
                out.push_str(&plain(
                    "  a major says existing references may no longer hold, which is a \
                     claim only you can make\n",
                ));
                for (label, ids) in [
                    ("removed", &self.classification.removed),
                    ("retired", &self.classification.retired),
                ] {
                    for id in ids {
                        out.push_str(&format!("  {} {}\n", dim(label), plain(id)));
                    }
                }
                out.push_str(&dim(
                    "  re-run with `--major --notes <file>` once you have written what it \
                     invalidates\n",
                ));
                return out.trim_end().to_string();
            }
            ReleaseStatus::Planned => {
                out.push_str(&bold(&format!(
                    "would release {} {}\n",
                    self.package, self.version
                )));
            }
            ReleaseStatus::Released => {
                out.push_str(&green(&format!(
                    "✓ released {} {}\n",
                    self.package, self.version
                )));
            }
        }
        let bump = match self.bump {
            Some(bump) => format!("{bump} — {}", self.classification.evidence()),
            None => format!("first release — {}", self.classification.evidence()),
        };
        kv(&mut out, "bump", 9, &bump);
        if let Some(tag) = &self.tag {
            kv(&mut out, "tag", 9, tag);
        }
        if let Some(record) = &self.record {
            kv(&mut out, "record", 9, record);
        }
        if let Some(commit) = &self.commit {
            kv(&mut out, "commit", 9, &commit[..commit.len().min(12)]);
        }
        for (label, ids) in [
            ("added", &self.classification.added),
            ("changed", &self.classification.changed),
            ("retired", &self.classification.retired),
            ("removed", &self.classification.removed),
        ] as [(&str, &Vec<String>); 4]
        {
            for id in ids.iter().take(10) {
                out.push_str(&format!("  {} {}\n", dim(label), plain(id)));
            }
            if ids.len() > 10 {
                out.push_str(&dim(&format!("  {label} … and {} more\n", ids.len() - 10)));
            }
        }
        for cited in &self.advisories {
            out.push_str(&yellow(&format!(
                "  {} has {} inbound references — patch, really?\n",
                cited.id, cited.inbound
            )));
        }
        if let Some(warnings) = self.warnings
            && warnings > 0
        {
            out.push_str(&yellow(&format!(
                "  {} — `vaire check` for the detail\n",
                pluralize(warnings, "warning")
            )));
        }
        if self.status == ReleaseStatus::Released {
            out.push_str(&dim("  push the commit and tag when you are ready\n"));
        }
        out.trim_end().to_string()
    }
}

/// The release `vaire status` says is pending — the ambient half of the classifier, so a
/// release is never a surprise (registry.v2.md §3.1).
#[derive(Debug, Serialize)]
pub struct PendingRelease {
    /// The last released version's tag, absent when the package has never been released.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// What a release would be: `initial`, `none`, or a bump.
    pub would_be: String,
    pub added: usize,
    pub changed: usize,
    pub retired: usize,
    pub removed: usize,
}

impl PendingRelease {
    pub fn from(
        classification: crate::release::classify::Classification,
        since: Option<String>,
    ) -> PendingRelease {
        use crate::release::classify::Outcome;
        PendingRelease {
            since,
            would_be: match classification.outcome {
                Outcome::Initial => "initial".to_string(),
                Outcome::Nothing => "none".to_string(),
                Outcome::Bump(bump) => bump.to_string(),
            },
            added: classification.added.len(),
            changed: classification.changed.len(),
            retired: classification.retired.len(),
            removed: classification.removed.len(),
        }
    }

    fn render(&self) -> String {
        match self.would_be.as_str() {
            "none" => "up to date".to_string(),
            "initial" => {
                "never released — `vaire release` publishes the declared version".to_string()
            }
            bump => {
                let mut parts = Vec::new();
                for (n, label) in [
                    (self.added, "new"),
                    (self.changed, "changed"),
                    (self.retired, "retired"),
                    (self.removed, "removed"),
                ] {
                    if n > 0 {
                        parts.push(format!("{n} {label}"));
                    }
                }
                format!("would be {bump} — {}", parts.join(", "))
            }
        }
    }
}

// ---- vaire catalog ---------------------------------------------------------

/// `vaire catalog list` (cli.md §4.8).
#[derive(Debug, Serialize)]
pub struct CatalogListOutput {
    pub catalog: String,
    pub sightings: Vec<crate::catalog::Sighting>,
}

/// `vaire catalog add` / `scan` — what got recorded.
#[derive(Debug, Serialize)]
pub struct CatalogRecordOutput {
    pub catalog: String,
    pub recorded: Vec<crate::catalog::Sighting>,
    /// The directory a `scan` walked; absent for a single `add`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanned: Option<String>,
    /// Directories holding an unreadable manifest — surfaced so a malformed one does not
    /// look like an absent package.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
    /// The walk stopped early, so "not found" may be wrong.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// `vaire catalog rm`.
#[derive(Debug, Serialize)]
pub struct CatalogRemoveOutput {
    pub catalog: String,
    pub removed: usize,
    pub target: String,
    /// This was the `--missing` sweep rather than a named removal — which reads
    /// differently when it matches nothing, since a clean catalog hits that every time.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub swept: bool,
}

impl Output for CatalogListOutput {
    fn render_human(&self) -> String {
        if self.sightings.is_empty() {
            return format!(
                "{}\n{}",
                dim("the catalog is empty"),
                dim(
                    "  `vaire catalog scan <dir>` imports a tree of packages; `index` and `check` record what they touch"
                ),
            );
        }
        let mut out = format!("{}\n", pluralize(self.sightings.len(), "package"));
        let w = col_width(self.sightings.iter().map(|s| s.name.as_str()));
        for s in &self.sightings {
            let state = match s.state {
                crate::catalog::State::Live => dim("live").to_string(),
                crate::catalog::State::Missing => yellow("missing").to_string(),
            };
            out.push_str(&format!(
                "  {}  {}  {}  {}  {}\n",
                cyan(&format!("{:<w$}", s.name)),
                plain(&format!("{:<9}", s.version)),
                state,
                dim(&format!("{:<10}", s.origin.as_str())),
                dim(&s.path.display().to_string()),
            ));
        }
        out.trim_end().to_string()
    }
}

impl Output for CatalogRecordOutput {
    fn render_human(&self) -> String {
        let mut out = match &self.scanned {
            Some(dir) => format!(
                "{}\n",
                green(&format!(
                    "✓ recorded {} from {dir}",
                    pluralize(self.recorded.len(), "package")
                ))
            ),
            None => match self.recorded.first() {
                Some(s) => format!(
                    "{}\n",
                    green(&format!("✓ recorded {} {}", s.name, s.version))
                ),
                None => format!("{}\n", yellow("nothing recorded")),
            },
        };
        if self.scanned.is_some() {
            for s in &self.recorded {
                out.push_str(&format!(
                    "  {}  {}\n",
                    cyan(&s.name),
                    dim(&s.path.display().to_string())
                ));
            }
        } else if let Some(s) = self.recorded.first() {
            out.push_str(&format!("  {}\n", dim(&s.path.display().to_string())));
        }
        for note in &self.unreadable {
            out.push_str(&yellow(&format!("  skipped: {note}\n")));
        }
        if self.truncated {
            out.push_str(&yellow(
                "  the walk stopped early — narrow the directory you scanned\n",
            ));
        }
        out.trim_end().to_string()
    }
}

impl Output for CatalogRemoveOutput {
    fn render_human(&self) -> String {
        match (self.removed, self.swept) {
            (0, true) => dim("every sighting still answers — nothing to sweep").to_string(),
            (0, false) => {
                dim(&format!("nothing in the catalog matched {}", self.target)).to_string()
            }
            (n, _) => green(&format!("✓ forgot {}", pluralize(n, "sighting"))).to_string(),
        }
    }
}
