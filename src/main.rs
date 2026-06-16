//! `vaire` binary entry point.
//!
//! Thin by design: parse the CLI, build the per-invocation context, dispatch to a
//! command, render its output (human or `--json`), and map the outcome to one of the
//! documented exit codes (cli.md §7). All real work lives in the `vaire` library crate.

use std::process::ExitCode as ProcExitCode;

use vaire::cli::{Cli, Command};
use vaire::commands::{self, Ctx};
use vaire::error::{ExitCode, VaireError};
use vaire::output::Output;
use vaire::{Result, mcp};

use clap::Parser;

fn main() -> ProcExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    vaire::output::init_color(cli.no_color);

    let code = match dispatch(cli) {
        Ok(code) => code,
        Err(err) => {
            emit_error(&err, json);
            err.exit_code()
        }
    };
    // Map our typed exit code onto the process exit status.
    ProcExitCode::from(code.code() as u8)
}

/// Run the requested command and return the exit code on success paths (including
/// `check`'s non-clean exit `6`, which is a successful run with a non-zero status).
fn dispatch(cli: Cli) -> Result<ExitCode> {
    let json = cli.json;

    // `init` scaffolds the corpus, so it runs *before* discovery (which needs `.vaire/`).
    if let Command::Init { path } = &cli.command {
        emit(&commands::init::run(path.as_deref())?, json);
        return Ok(ExitCode::Success);
    }

    // `mcp` builds its own long-lived context and never returns output.
    if let Command::Mcp = cli.command {
        let ctx = Ctx::new(cli.repo, cli.config)?;
        mcp::serve(ctx)?;
        return Ok(ExitCode::Success);
    }

    let ctx = Ctx::new(cli.repo, cli.config)?;

    match cli.command {
        Command::Resolve { id } => {
            emit(&commands::resolve::run(&ctx, &id)?, json);
        }
        Command::Render { id } => {
            emit(&commands::render::run(&ctx, &id)?, json);
        }
        Command::Backlinks {
            id,
            type_filter,
            limit,
        } => {
            let out = commands::backlinks::run(&ctx, &id, type_filter.as_deref(), limit)?;
            emit(&out, json);
        }
        Command::Refs {
            id,
            depth,
            type_filter,
        } => {
            let out = commands::refs::run(&ctx, &id, depth, type_filter.as_deref())?;
            emit(&out, json);
        }
        Command::Search {
            query,
            type_filter,
            scope,
            limit,
        } => {
            let out = commands::search::run(
                &ctx,
                &query,
                type_filter.as_deref(),
                scope.as_deref(),
                Some(limit),
            )?;
            emit(&out, json);
        }
        Command::Suggest {
            descriptor,
            type_filter,
            limit,
        } => {
            let out =
                commands::suggest::run(&ctx, &descriptor, type_filter.as_deref(), Some(limit))?;
            emit(&out, json);
        }
        Command::Unresolved { type_filter, scope } => {
            let out = commands::unresolved::run(&ctx, type_filter.as_deref(), scope.as_deref())?;
            emit(&out, json);
        }
        Command::Index {
            full,
            working_tree,
            re_embed,
        } => {
            emit(
                &commands::index::run(&ctx, full, working_tree, re_embed)?,
                json,
            );
        }
        Command::Check {
            strict,
            working_tree,
        } => {
            let (report, failed) = commands::check::run(&ctx, strict, working_tree)?;
            emit(&report, json);
            if failed {
                return Ok(ExitCode::CheckViolations);
            }
        }
        Command::Status => {
            emit(&commands::status::run(&ctx)?, json);
        }
        Command::Init { .. } | Command::Mcp => unreachable!("handled above"),
    }

    Ok(ExitCode::Success)
}

/// Write a command result to stdout — JSON or human text (cli.md §2.3).
fn emit<O: Output>(out: &O, json: bool) {
    if json {
        println!("{}", out.to_json());
    } else {
        println!("{}", out.render_human());
    }
}

/// Write an error: the `{"error": {...}}` JSON shape on stdout under `--json`, or a
/// plain message on stderr otherwise (cli.md §7).
fn emit_error(err: &VaireError, json: bool) {
    if json {
        println!("{}", err.to_json());
    } else {
        eprintln!("error: {err}");
    }
}
