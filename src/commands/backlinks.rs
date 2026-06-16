//! `vaire backlinks <id> [--type T] [--limit N]` (cli.md §3.2).

use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::model::id::{NodeId, NodeType};
use crate::output::{BacklinksOutput, EdgeRef};

pub fn run(
    ctx: &Ctx,
    id: &str,
    type_filter: Option<&str>,
    limit: Option<usize>,
) -> Result<BacklinksOutput> {
    let id: NodeId = id
        .parse()
        .map_err(|e| VaireError::Usage(format!("bad id '{id}': {e}")))?;
    let ty = type_filter.map(NodeType::new);
    let index = ctx.open_index()?;
    let rows = index.backlinks(&id, ty.as_ref(), limit)?;
    let backlinks: Vec<EdgeRef> = rows
        .into_iter()
        .map(|r| EdgeRef {
            id: r.id.to_string(),
            node_type: r.node_type.to_string(),
            path: r.path,
            ref_type: r.ref_type,
            line: r.line,
            distance: None, // backlinks omit distance
        })
        .collect();
    Ok(BacklinksOutput {
        id: id.to_string(),
        count: backlinks.len(),
        backlinks,
    })
}
