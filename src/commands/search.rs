//! `vaire search <query> [--type T] [--scope project-id] [--limit N]` (cli.md §3.4).

use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::model::id::{NodeId, NodeType};
use crate::output::{AnchorOut, SearchOutput, SearchResult};
use crate::search::{self, SearchOpts};

pub fn run(
    ctx: &Ctx,
    query: &str,
    type_filter: Option<&str>,
    scope: Option<&str>,
    limit: Option<usize>,
) -> Result<SearchOutput> {
    let opts = SearchOpts {
        type_filter: type_filter.map(NodeType::new),
        scope: scope
            .map(|s| s.parse::<NodeId>())
            .transpose()
            .map_err(|e| VaireError::Usage(format!("bad --scope: {e}")))?,
        limit,
        scope_field: ctx.config.scope_field.clone(),
    };
    let index = ctx.open_index()?;
    let embedder = ctx.embedder()?;
    let hits = search::search(&index, embedder.as_ref(), query, &opts)?;
    // With an explicit --scope, every result is in that scope, so the prefix is implied:
    // show the node's own `type:id`. Without it, show the full `scope/type:id`.
    let scoped_query = opts.scope.is_some();
    let results: Vec<SearchResult> = hits
        .into_iter()
        .map(|h| SearchResult {
            id: if scoped_query {
                h.id.local_id()
            } else {
                h.id.to_string()
            },
            node_type: h.node_type.to_string(),
            path: h.path,
            score: h.score,
            anchors: h
                .anchors
                .into_iter()
                .map(|a| AnchorOut {
                    heading: a.heading,
                    line: a.line,
                    snippet: a.snippet,
                })
                .collect(),
        })
        .collect();
    Ok(SearchOutput {
        query: query.to_string(),
        count: results.len(),
        results,
    })
}
