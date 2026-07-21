//! `vaire render <id>` — the node's Markdown with frontmatter kept and wikilinks
//! resolved to portable Markdown links (design.md §6).
//!
//! Frontmatter is emitted verbatim. In the prose, each resolved `[[type:id]]` /
//! `[[type:id|display]]` becomes `[display](relative-path)` — display from the `|`
//! override or the target's `name:`, href a path relative to this file. Unresolved
//! `[[?...]]` render as their plain descriptor (they are not links). Dangling targets
//! and wikilinks inside fenced code blocks are left verbatim.
//!
//! Cross-package (M5): the rendered node may itself live in a linked package, and its
//! inline references resolve **in that package's context** — an `@pkg/` ref inside an
//! acme-core file goes through acme-core's `[dependencies]`, not the run-root's
//! (design.md §9). Hrefs to another package are filesystem-relative paths through the
//! link (`../../acme-core/…`); same-package hrefs are byte-identical to before.

use std::rc::Rc;

use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::model::id::NodeId;
use crate::model::reference::Reference;
use crate::output::RenderOutput;
use crate::workspace::resolver::{self, Resolved};
use crate::workspace::{PackageHandle, Workspace};

pub fn run(ctx: &Ctx, id: &str) -> Result<RenderOutput> {
    let id: NodeId = id
        .parse()
        .map_err(|e| VaireError::Usage(format!("bad id '{id}': {e}")))?;
    let ws = ctx.workspace()?;
    let resolved = resolver::resolve(ws, ws.current(), &id)?; // exit 5 if not a node

    // The rendered node's owning package is the context every inline ref resolves in.
    let owner = if resolved.package == ws.current().id {
        ws.current()
    } else {
        ws.handle_at(&resolved.root).ok_or_else(|| {
            VaireError::Dependency(format!(
                "package '{}' disappeared during render",
                resolved.package
            ))
        })?
    };
    let source_path = resolved.node.path.clone();
    let source_scope = resolved.node.id.scope().map(str::to_string);

    let raw = std::fs::read_to_string(resolved.root.join(&source_path))?;
    let (header, prose) = split_raw(&raw);
    let body = render_prose(&prose, &source_path, source_scope.as_deref(), ws, &owner);

    let mut markdown = String::new();
    if !header.is_empty() {
        markdown.push_str(&header);
        markdown.push('\n');
    }
    markdown.push_str(&body);
    if !markdown.ends_with('\n') {
        markdown.push('\n');
    }

    let cross = resolved.package != ws.current().id;
    Ok(RenderOutput {
        id: resolved.node.id.to_string(),
        path: source_path,
        package: cross.then(|| resolved.package.to_string()),
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
    ws: &Workspace,
    owner: &Rc<PackageHandle>,
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
            out.push_str(&render_line(line, source_path, source_scope, ws, owner));
        }
        out.push('\n');
    }
    out
}

fn render_line(
    line: &str,
    source_path: &str,
    source_scope: Option<&str>,
    ws: &Workspace,
    owner: &Rc<PackageHandle>,
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
            ws,
            owner,
        ));
        rest = &after[end + 2..];
    }
    result.push_str(rest);
    result
}

/// Resolve a reference target **scope-first, then global** (cli.md §6.1): a bare target
/// inside a scoped node prefers a same-scope sibling; otherwise the global target.
/// Scope-first applies only within the owner's own package — an `@pkg/` target skips it.
/// Any resolution failure (dangling, unlinked dependency) yields `None` → verbatim.
fn resolve_scope_first(
    ws: &Workspace,
    owner: &Rc<PackageHandle>,
    target: &NodeId,
    source_scope: Option<&str>,
) -> Option<Resolved> {
    if let Some(scope) = source_scope
        && target.scope().is_none()
        && target.package().is_none()
    {
        let mut scoped = target.clone();
        scoped.scope = Some(scope.to_string());
        if let Ok(node) = resolver::resolve(ws, owner.clone(), &scoped) {
            return Some(node);
        }
    }
    resolver::resolve(ws, owner.clone(), target).ok()
}

fn render_ref(
    inner: &str,
    source_path: &str,
    source_scope: Option<&str>,
    ws: &Workspace,
    owner: &Rc<PackageHandle>,
) -> String {
    match Reference::parse_inner(inner) {
        Some(Reference::Resolved { target, display }) => {
            match resolve_scope_first(ws, owner, &target, source_scope) {
                Some(resolved) => {
                    let name = resolved
                        .node
                        .frontmatter
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| target.slug.clone());
                    let text = display.unwrap_or(name);
                    let href = if resolved.root == owner.root {
                        // Same package: byte-identical to the pre-M5 output.
                        relative_path(source_path, &resolved.node.path)
                    } else {
                        cross_package_href(owner, source_path, &resolved)
                    };
                    format!("[{text}]({href})")
                }
                None => format!("[[{inner}]]"), // dangling — keep verbatim
            }
        }
        // Loose ends are not links: render the author's descriptor as plain text.
        Some(Reference::Unresolved { descriptor, .. }) => descriptor,
        None => format!("[[{inner}]]"),
    }
}

/// Href from a file in `owner` to a node in another package: filesystem-relative through
/// the canonical roots (`../../acme-core/knowledge/….md`).
fn cross_package_href(owner: &PackageHandle, source_path: &str, resolved: &Resolved) -> String {
    let from_dir = owner
        .root
        .join(source_path)
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| owner.root.clone());
    let to = resolved.root.join(&resolved.node.path);
    match crate::workspace::relative_to(&to, &from_dir) {
        Some(rel) => rel.display().to_string(),
        None => to.display().to_string(),
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
