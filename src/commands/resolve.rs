//! `vaire resolve <id>` (cli.md §3.1) — cross-package since M5: an `@pkg/…` id routes
//! through the linked package (cli.md §6.5), and `superseded_by` chains may hop
//! packages. Local ids behave exactly as before.

use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::model::id::NodeId;
use crate::output::ResolveOutput;
use crate::workspace::resolver;

pub fn run(ctx: &Ctx, id: &str) -> Result<ResolveOutput> {
    let id: NodeId = id
        .parse()
        .map_err(|e| VaireError::Usage(format!("bad id '{id}': {e}")))?;
    crate::commands::require_qualified(ctx, &id)?;
    let ws = ctx.workspace()?;
    let resolved = resolver::resolve(ws, ws.current(), &id)?;

    let cross = resolved.package != ws.current().id;
    let display_path = cross.then(|| {
        crate::workspace::display_path(&ws.current().root, &resolved.root, &resolved.node.path)
    });
    Ok(ResolveOutput {
        id: resolved.node.id.to_string(),
        node_type: resolved.node.node_type.to_string(),
        path: resolved.node.path,
        package: cross.then(|| resolved.package.to_string()),
        frontmatter: resolved.node.frontmatter,
        requested_id: resolved.node.requested_id.map(|i| i.to_string()),
        superseded_by: resolved.node.superseded_by.map(|i| i.to_string()),
        display_path,
    })
}
