//! `vaire deps` (cli.md §3.8) — the resolved local dependency tree.
//!
//! Pure **live link inspection** (`.vaire/packages/`, cli.md §6.5): no index needed, so
//! it is a safe first command in a fresh workspace and always exits `0` — reporting is
//! its job, erroring is `vaire check`'s. Each member's own dependencies resolve through
//! *its* manifest and links (the same (source package, dep name) keying as resolution);
//! cycles are annotated once and not descended into. The `^N` satisfaction marker is
//! surfaced only — version *enforcement* is v0.3.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;

use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::model::Version;
use crate::output::{DepNode, DepsOutput};
use crate::workspace::{PackageHandle, Workspace};

pub fn run(ctx: &Ctx) -> Result<DepsOutput> {
    let ws = ctx.workspace()?;
    // Unlike the fan-out reads, this one has no rootless form to widen into: it reports
    // *a package's* links, and the synthetic root has neither a name nor a `.vaire/`. The
    // machine-wide question it looks like it might answer is a different command's.
    if ws.is_rootless() {
        return Err(VaireError::Usage(
            "`deps` reports the dependency tree of the package you are standing in, and \
             there is none here — run it inside a package, or use `vaire catalog list` to \
             see what this machine knows"
                .into(),
        ));
    }
    let current = ws.current();
    let mut visited: BTreeSet<PathBuf> = [current.root.clone()].into();
    let dependencies = children(ws, &current, &current, &mut visited);
    Ok(DepsOutput {
        name: ctx.config.name.clone(),
        version: ctx.config.version.clone(),
        dependencies,
    })
}

/// The dependency nodes of `pkg`, resolved through ITS manifest and links. `visited`
/// tracks canonical roots already on the path from the run-root, so a cycle is annotated
/// (once) instead of descended into.
fn children(
    ws: &Workspace,
    pkg: &Rc<PackageHandle>,
    run_root: &Rc<PackageHandle>,
    visited: &mut BTreeSet<PathBuf>,
) -> Vec<DepNode> {
    let mut out = Vec::new();
    for (name, constraint) in &pkg.config.dependencies {
        match ws.locate(pkg, name) {
            Err(e) => out.push(DepNode {
                name: name.clone(),
                constraint: constraint.clone(),
                version: None,
                resolved: None,
                satisfied: None,
                cycle: false,
                note: Some(e.to_string()),
                dependencies: Vec::new(),
            }),
            Ok(handle) => {
                let resolved = if handle.root == run_root.root {
                    ".".to_string() // a cycle back into the package we're standing in
                } else {
                    crate::workspace::relative_to(&handle.root, &run_root.root)
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| handle.root.display().to_string())
                };
                // Parsed, not string-compared: `^1` against a declared `1.10.0` is a
                // numeric question, and the textual form got `^1` vs `01` subtly wrong.
                let satisfied = handle
                    .config
                    .version
                    .parse::<Version>()
                    .ok()
                    .map(|version| version.satisfies_caret(constraint));
                let cycle = !visited.insert(handle.root.clone());
                let dependencies = if cycle {
                    Vec::new()
                } else {
                    let nested = children(ws, &handle, run_root, visited);
                    visited.remove(&handle.root);
                    nested
                };
                out.push(DepNode {
                    name: name.clone(),
                    constraint: constraint.clone(),
                    version: Some(handle.config.version.clone()),
                    resolved: Some(resolved),
                    satisfied,
                    cycle,
                    note: None,
                    dependencies,
                });
            }
        }
    }
    out
}
