//! `vaire search <query> [--type T] [--scope id] [--limit N] [--local]` (cli.md §3.4)
//! — cross-package since M5: the query runs over the run-root + its dependency closure
//! (embedded once), `--local` restricts to this package, and `--scope @pkg/container`
//! searches inside a dependency's container.

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
    local: bool,
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
    let ws = ctx.workspace()?;
    let embedder = ctx.embedder()?;
    let (hits, skipped) = search::search_workspace(ws, embedder.as_ref(), query, &opts, local)?;

    let run_root = ws.current().root.clone();
    // With an explicit --scope, every result is in that scope, so the prefix is implied:
    // show the node's own `type:id`. Without it, show the full (qualified) id.
    let scoped_query = opts.scope.is_some();
    let results: Vec<SearchResult> = hits
        .into_iter()
        .map(|h| SearchResult {
            id: if scoped_query {
                h.hit.id.local_id()
            } else {
                h.hit.id.to_string()
            },
            node_type: h.hit.node_type.to_string(),
            display_path: h
                .package
                .is_some()
                .then(|| crate::workspace::display_path(&run_root, &h.root, &h.hit.path)),
            path: h.hit.path,
            package: h.package,
            score: h.hit.score,
            anchors: h
                .hit
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
        skipped,
    })
}
