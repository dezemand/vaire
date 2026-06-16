//! `vaire suggest <descriptor>` (cli.md §3.7) — the lookup-before-reference primitive.
//!
//! Given a free-text descriptor of something you want to reference, returns ranked
//! existing node IDs it might be (matched on `name`/`aliases` first, prose FTS as backup —
//! design.md §8). The authoring step before writing `[[type:id]]` or, when nothing fits,
//! `[[?type: descriptor]]`.

use crate::commands::Ctx;
use crate::error::Result;
use crate::model::id::NodeType;
use crate::output::{SuggestOutput, SuggestionItem};
use crate::search;

pub fn run(
    ctx: &Ctx,
    descriptor: &str,
    type_filter: Option<&str>,
    limit: Option<usize>,
) -> Result<SuggestOutput> {
    let ty = type_filter.map(NodeType::new);
    let index = ctx.open_index()?;
    let found = search::suggest(&index, descriptor, ty.as_ref(), limit.unwrap_or(5))?;
    let suggestions: Vec<SuggestionItem> = found
        .into_iter()
        .map(|s| SuggestionItem {
            id: s.id.to_string(),
            node_type: s.node_type.to_string(),
            name: s.name,
            path: s.path,
            score: s.score,
        })
        .collect();
    Ok(SuggestOutput {
        descriptor: descriptor.to_string(),
        count: suggestions.len(),
        suggestions,
    })
}
