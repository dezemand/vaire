//! `vaire refs <id> [--depth N] [--type T]` (cli.md §3.3).
//!
//! Unresolved (`[[?...]]`) references are not edges and never appear here — use
//! `vaire unresolved`.

use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::model::id::{NodeId, NodeType};
use crate::output::{EdgeRef, RefsOutput};

pub fn run(ctx: &Ctx, id: &str, depth: u32, type_filter: Option<&str>) -> Result<RefsOutput> {
    let id: NodeId = id
        .parse()
        .map_err(|e| VaireError::Usage(format!("bad id '{id}': {e}")))?;
    let ty = type_filter.map(NodeType::new);
    let index = ctx.open_index()?;
    let rows = index.refs(&id, depth, ty.as_ref())?;
    let refs: Vec<EdgeRef> = rows
        .into_iter()
        .map(|r| EdgeRef {
            id: r.id.to_string(),
            node_type: r.node_type.to_string(),
            path: r.path,
            ref_type: r.ref_type,
            line: r.line,
            distance: Some(r.distance),
        })
        .collect();
    Ok(RefsOutput {
        id: id.to_string(),
        depth,
        count: refs.len(),
        refs,
    })
}
