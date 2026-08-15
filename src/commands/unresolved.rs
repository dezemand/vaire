//! `vaire unresolved [--type T] [--scope container-id] [--all-packages]` (cli.md §3.5).
//!
//! The work list for the entity-creation pass (design.md §8), derived fresh from the
//! files on each call — there is no stored queue. Default scope is the CURRENT package:
//! a descriptor is package-agnostic (packages.md §7) and a dependency's loose ends are
//! its owner's worklist. `--all-packages` widens to the linked closure, each row tagged
//! with its package.

use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::model::id::{NodeId, NodeType};
use crate::output::{UnresolvedItem, UnresolvedOutput};

pub fn run(
    ctx: &Ctx,
    type_filter: Option<&str>,
    scope: Option<&str>,
    all_packages: bool,
) -> Result<UnresolvedOutput> {
    let ty = type_filter.map(NodeType::new);
    let scope = scope
        .map(|s| s.parse::<NodeId>())
        .transpose()
        .map_err(|e| VaireError::Usage(format!("bad --scope: {e}")))?;
    if all_packages && scope.is_some() {
        return Err(VaireError::Usage(
            "--scope cannot be combined with --all-packages (a scope is one package's container)"
                .into(),
        ));
    }

    let ws = ctx.workspace()?;
    // A rootless session (cli.md §6.8) has no current package, so the default scope has
    // nothing to mean and the widened form is the only one there is. Taken here rather
    // than left to fail later: asking the synthetic root for an index would report a
    // missing index, which names neither the situation nor anything the caller can fix.
    let all_packages = all_packages || ws.is_rootless();
    if ws.is_rootless() && scope.is_some() {
        return Err(VaireError::Usage(
            "--scope needs a package to be relative to, and there is none here — run this \
             inside a package, or drop --scope to list every unresolved reference in your \
             catalog"
                .into(),
        ));
    }
    let run_root = ws.current();
    let (members, mut skipped) = if all_packages {
        ws.consult_closure()
    } else {
        (vec![run_root.clone()], Vec::new())
    };

    let mut unresolved: Vec<UnresolvedItem> = Vec::new();
    for member in members {
        let is_run_root = member.root == run_root.root;
        let index = match member.index() {
            Ok(index) => index,
            Err(e) if is_run_root => return Err(e),
            Err(_) => {
                skipped.push(member.id.to_string());
                continue;
            }
        };
        let rows = index.unresolved(ty.as_ref(), scope.as_ref(), &member.config.scope_field)?;
        for r in rows {
            let record = if is_run_root {
                r.record.to_string()
            } else {
                r.record
                    .clone()
                    .with_package(member.id.0.clone())
                    .to_string()
            };
            unresolved.push(UnresolvedItem {
                record,
                display_path: (!is_run_root)
                    .then(|| crate::workspace::display_path(&run_root.root, &member.root, &r.path)),
                path: r.path,
                package: (!is_run_root).then(|| member.id.to_string()),
                type_guess: r.type_guess.map(|t| t.to_string()),
                descriptor: r.descriptor,
                line: r.line,
            });
        }
    }

    skipped.sort();
    skipped.dedup();
    Ok(UnresolvedOutput {
        count: unresolved.len(),
        unresolved,
        skipped,
    })
}
