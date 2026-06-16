//! `vaire unresolved [--type T] [--scope project-id]` (cli.md §3.5).
//!
//! The work list for the entity-creation pass (design.md §8), derived fresh from the
//! files on each call — there is no stored queue.

use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::model::id::{NodeId, NodeType};
use crate::output::{UnresolvedItem, UnresolvedOutput};

pub fn run(ctx: &Ctx, type_filter: Option<&str>, scope: Option<&str>) -> Result<UnresolvedOutput> {
    let ty = type_filter.map(NodeType::new);
    let scope = scope
        .map(|s| s.parse::<NodeId>())
        .transpose()
        .map_err(|e| VaireError::Usage(format!("bad --scope: {e}")))?;
    let index = ctx.open_index()?;
    let rows = index.unresolved(ty.as_ref(), scope.as_ref(), &ctx.config.scope_field)?;
    let unresolved: Vec<UnresolvedItem> = rows
        .into_iter()
        .map(|r| UnresolvedItem {
            record: r.record.to_string(),
            path: r.path,
            type_guess: r.type_guess.map(|t| t.to_string()),
            descriptor: r.descriptor,
            line: r.line,
        })
        .collect();
    Ok(UnresolvedOutput {
        count: unresolved.len(),
        unresolved,
    })
}
