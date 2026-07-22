//! `vaire suggest <descriptor> [--local]` (cli.md §3.7) — the lookup-before-reference
//! primitive.
//!
//! Given a free-text descriptor of something you want to reference, returns ranked
//! existing node IDs it might be (matched on `name`/`aliases` first, prose FTS as backup —
//! design.md §8). The authoring step before writing `[[type:id]]` or, when nothing fits,
//! `[[?type: descriptor]]`. Cross-package since M5: suggestions come from the run-root +
//! its dependency closure (a dep hit arrives pre-qualified, ready to paste as
//! `[[@pkg/type:id]]`); `--local` restricts to this package.

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
    local: bool,
) -> Result<SuggestOutput> {
    let ty = type_filter.map(NodeType::new);
    let ws = ctx.workspace()?;
    let (found, skipped) =
        search::suggest_workspace(ws, descriptor, ty.as_ref(), limit.unwrap_or(5), local)?;

    let run_root = ws.current().root.clone();
    let suggestions: Vec<SuggestionItem> = found
        .into_iter()
        .map(|s| SuggestionItem {
            id: s.suggestion.id.to_string(),
            node_type: s.suggestion.node_type.to_string(),
            name: s.suggestion.name,
            display_path: s
                .package
                .is_some()
                .then(|| crate::workspace::display_path(&run_root, &s.root, &s.suggestion.path)),
            path: s.suggestion.path,
            package: s.package,
            score: s.suggestion.score,
        })
        .collect();
    Ok(SuggestOutput {
        descriptor: descriptor.to_string(),
        count: suggestions.len(),
        suggestions,
        skipped,
    })
}
