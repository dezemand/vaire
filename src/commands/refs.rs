//! `vaire refs <id> [--depth N] [--type T]` (cli.md §3.3) — cross-package since M5:
//! the BFS follows `@pkg/` edges through each edge's OWNING package (source-package
//! keying), so depth-2 traversals cross boundaries and come back.
//!
//! Unresolved (`[[?...]]`) references are not edges and never appear here — use
//! `vaire unresolved`.

use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::model::id::{NodeId, NodeType};
use crate::output::{EdgeRef, RefsOutput};
use crate::workspace::resolver;

pub fn run(ctx: &Ctx, id: &str, depth: u32, type_filter: Option<&str>) -> Result<RefsOutput> {
    let id: NodeId = id
        .parse()
        .map_err(|e| VaireError::Usage(format!("bad id '{id}': {e}")))?;
    crate::commands::require_qualified(ctx, &id)?;
    let ty = type_filter.map(NodeType::new);
    let ws = ctx.workspace()?;
    let cross = resolver::refs(ws, &id, depth, ty.as_ref())?;

    let run_root = ws.current().root.clone();
    let refs: Vec<EdgeRef> = cross
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
            distance: Some(m.row.distance),
        })
        .collect();
    Ok(RefsOutput {
        rootless: ws.is_rootless(),
        id: id.to_string(),
        depth,
        count: refs.len(),
        refs,
        skipped: cross.skipped,
    })
}
