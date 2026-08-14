//! `vaire backlinks <id> [--type T] [--limit N]` (cli.md §3.2) — cross-package since
//! M5: inbound edges are gathered from the whole dependency closure, each member
//! consulted through its own aliases for the target's package.

use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::model::id::{NodeId, NodeType};
use crate::output::{BacklinksOutput, EdgeRef};
use crate::workspace::resolver;

pub fn run(
    ctx: &Ctx,
    id: &str,
    type_filter: Option<&str>,
    limit: Option<usize>,
) -> Result<BacklinksOutput> {
    let id: NodeId = id
        .parse()
        .map_err(|e| VaireError::Usage(format!("bad id '{id}': {e}")))?;
    crate::commands::require_qualified(ctx, &id)?;
    let ty = type_filter.map(NodeType::new);
    let ws = ctx.workspace()?;
    let cross = resolver::backlinks(ws, &id, ty.as_ref(), limit)?;

    let run_root = ws.current().root.clone();
    let backlinks: Vec<EdgeRef> = cross
        .rows
        .into_iter()
        .map(|m| EdgeRef {
            id: m.row.id.to_string(),
            node_type: m.row.node_type.to_string(),
            display_path: m
                .package
                .is_some()
                .then(|| crate::workspace::display_path(&run_root, &m.root, &m.row.path)),
            path: m.row.path,
            package: m.package.as_ref().map(|p| p.to_string()),
            ref_type: m.row.ref_type,
            line: m.row.line,
            distance: None, // backlinks omit distance
        })
        .collect();
    Ok(BacklinksOutput {
        rootless: ws.is_rootless(),
        id: id.to_string(),
        count: backlinks.len(),
        backlinks,
        skipped: cross.skipped,
    })
}
