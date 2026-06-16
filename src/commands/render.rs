//! `vaire render <id>` — the node's Markdown with frontmatter kept and wikilinks
//! resolved to portable Markdown links (design.md §6).
//!
//! Frontmatter is emitted verbatim. In the prose, each resolved `[[type:id]]` /
//! `[[type:id|display]]` becomes `[display](relative-path)` — display from the `|`
//! override or the target's `name:`, href a path relative to this file. Unresolved
//! `[[?...]]` render as their plain descriptor (they are not links). Dangling targets
//! and wikilinks inside fenced code blocks are left verbatim.

use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::index::Index;
use crate::model::id::NodeId;
use crate::model::reference::Reference;
use crate::output::RenderOutput;

pub fn run(ctx: &Ctx, id: &str) -> Result<RenderOutput> {
    let id: NodeId = id
        .parse()
        .map_err(|e| VaireError::Usage(format!("bad id '{id}': {e}")))?;
    let index = ctx.open_index()?;
    let resolved = index.resolve(&id)?; // exit 5 if not a node; follows superseded_by
    let source_path = resolved.path;

    let source_scope = resolved.id.scope().map(str::to_string);

    let raw = std::fs::read_to_string(ctx.repo.root().join(&source_path))?;
    let (header, prose) = split_raw(&raw);
    let body = render_prose(
        &prose,
        &source_path,
        source_scope.as_deref(),
        &ctx.config.scoped_types,
        &index,
    );

    let mut markdown = String::new();
    if !header.is_empty() {
        markdown.push_str(&header);
        markdown.push('\n');
    }
    markdown.push_str(&body);
    if !markdown.ends_with('\n') {
        markdown.push('\n');
    }

    Ok(RenderOutput {
        id: resolved.id.to_string(),
        path: source_path,
        markdown,
    })
}

/// Split raw file text into the verbatim frontmatter block (incl. fences) and the prose.
fn split_raw(raw: &str) -> (String, String) {
    let lines: Vec<&str> = raw.lines().collect();
    if lines.first().map(|l| l.trim()) != Some("---") {
        return (String::new(), raw.to_string());
    }
    match lines.iter().skip(1).position(|l| l.trim() == "---") {
        Some(offset) => {
            let close = offset + 1; // index of the closing fence
            (lines[..=close].join("\n"), lines[close + 1..].join("\n"))
        }
        None => (String::new(), raw.to_string()), // no closing fence ⇒ all prose
    }
}

/// Rewrite wikilinks in prose, skipping fenced code blocks (consistent with indexing).
fn render_prose(
    prose: &str,
    source_path: &str,
    source_scope: Option<&str>,
    scoped_types: &[String],
    index: &Index,
) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    for line in prose.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_fence {
            out.push_str(line);
        } else {
            out.push_str(&render_line(
                line,
                source_path,
                source_scope,
                scoped_types,
                index,
            ));
        }
        out.push('\n');
    }
    out
}

fn render_line(
    line: &str,
    source_path: &str,
    source_scope: Option<&str>,
    scoped_types: &[String],
    index: &Index,
) -> String {
    let mut result = String::new();
    let mut rest = line;
    while let Some(start) = rest.find("[[") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            result.push_str(&rest[start..]); // unterminated — keep verbatim
            return result;
        };
        result.push_str(&render_ref(
            &after[..end],
            source_path,
            source_scope,
            scoped_types,
            index,
        ));
        rest = &after[end + 2..];
    }
    result.push_str(rest);
    result
}

fn render_ref(
    inner: &str,
    source_path: &str,
    source_scope: Option<&str>,
    scoped_types: &[String],
    index: &Index,
) -> String {
    match Reference::parse_inner(inner) {
        Some(Reference::Resolved {
            mut target,
            display,
        }) => {
            // Expand a relative scoped ref against the rendering node's own scope.
            if let Some(scope) = source_scope
                && scoped_types.iter().any(|t| t == target.node_type.as_str())
                && target.scope().is_none()
            {
                target.scope = Some(scope.to_string());
            }
            match index.resolve(&target) {
                Ok(node) => {
                    let name = node
                        .frontmatter
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| target.slug.clone());
                    let text = display.unwrap_or(name);
                    let href = relative_path(source_path, &node.path);
                    format!("[{text}]({href})")
                }
                Err(_) => format!("[[{inner}]]"), // dangling — keep verbatim
            }
        }
        // Loose ends are not links: render the author's descriptor as plain text.
        Some(Reference::Unresolved { descriptor, .. }) => descriptor,
        None => format!("[[{inner}]]"),
    }
}

/// POSIX path of `to_file` relative to the directory of `from_file` (both repo-relative).
fn relative_path(from_file: &str, to_file: &str) -> String {
    let from: Vec<&str> = from_file.split('/').collect();
    let from_dir = &from[..from.len().saturating_sub(1)];
    let to: Vec<&str> = to_file.split('/').collect();
    let to_dir_len = to.len().saturating_sub(1);

    let mut common = 0;
    while common < from_dir.len() && common < to_dir_len && from_dir[common] == to[common] {
        common += 1;
    }
    let ups = from_dir.len() - common;
    let mut parts: Vec<&str> = std::iter::repeat_n("..", ups).collect();
    parts.extend_from_slice(&to[common..]);
    let rel = parts.join("/");
    if ups == 0 { format!("./{rel}") } else { rel }
}
