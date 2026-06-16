//! `vaire resolve <id>` (cli.md §3.1).

use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::model::id::NodeId;
use crate::output::ResolveOutput;

pub fn run(ctx: &Ctx, id: &str) -> Result<ResolveOutput> {
    let id: NodeId = id
        .parse()
        .map_err(|e| VaireError::Usage(format!("bad id '{id}': {e}")))?;
    let index = ctx.open_index()?;
    let resolved = index.resolve(&id)?;
    Ok(ResolveOutput {
        id: resolved.id.to_string(),
        node_type: resolved.node_type.to_string(),
        path: resolved.path,
        frontmatter: resolved.frontmatter,
        requested_id: resolved.requested_id.map(|i| i.to_string()),
        superseded_by: resolved.superseded_by.map(|i| i.to_string()),
    })
}
