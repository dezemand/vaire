//! `vaire` binary entry point.
//!
//! Thin by design: parse the CLI, build the per-invocation context, dispatch to a
//! command, render its output (human or `--json`), and map the outcome to one of the
//! documented exit codes (cli.md §7). All real work lives in the `vaire` library crate.

use std::io::Read;
use std::process::ExitCode as ProcExitCode;

use vaire::cli::{Cli, Command, ConfigureSection};
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
    // Its target is the positional path if given, else the `--repo`/`VAIRE_REPO` override,
    // else the current directory.
    if let Command::Init { path } = &cli.command {
        let target = path.as_deref().or(cli.repo.as_deref());
        emit(&commands::init::run(target)?, json);
        return Ok(ExitCode::Success);
    }

    // `configure` writes the global user config; corpus-independent, so no discovery.
    // A section runs non-interactively; bare `configure` opens the guided prompt.
    if let Command::Configure { section } = &cli.command {
        let home = vaire::userconfig::config_home();
        let out = match section {
            Some(ConfigureSection::Embeddings {
                provider,
                model,
                dimensions,
                command,
                api_key_stdin,
                api_url,
            }) => {
                let opts = commands::configure::ConfigureOpts {
                    provider: provider.clone(),
                    model: model.clone(),
                    dimensions: *dimensions,
                    command: command.clone(),
                    api_key: (*api_key_stdin).then(read_api_key_from_stdin).transpose()?,
                    api_url: api_url.clone(),
                };
                commands::configure::run(&home, opts)?
            }
            Some(ConfigureSection::LocalPackages { path, unset }) => {
                commands::configure::run_local_packages(&home, path.as_deref(), *unset)?
            }
            None => commands::configure::run_interactive(&home)?,
        };
        emit(&out, json);
        return Ok(ExitCode::Success);
    }

    // `upgrade` operates on the binary itself — corpus-independent, no discovery.
    if let Command::Upgrade { version, check } = &cli.command {
        emit(&commands::upgrade::run(version.as_deref(), *check)?, json);
        return Ok(ExitCode::Success);
    }

    // `add` edits the manifest's [dependencies] (and with --link, the package links);
    // it needs the package root but not the index, so it runs before `Ctx` is built
    // (like `init`/`configure`).
    if let Command::Add { spec, link } = &cli.command {
        let out = commands::add::run(
            cli.repo.as_deref(),
            cli.config.as_deref(),
            spec,
            link.as_deref(),
        )?;
        emit(&out, json);
        return Ok(ExitCode::Success);
    }

    // `mcp` builds its own long-lived context and never returns output.
    if let Command::Mcp = cli.command {
        let ctx = Ctx::new(cli.repo, cli.config)?;
        mcp::serve(ctx)?;
        return Ok(ExitCode::Success);
    }

    // `pack` refuses `--config` before any context is built: the artifact's identity
    // and file selection must come from the package's own committed knowledge.toml, and
    // a rejected override should not even be loaded.
    #[cfg(feature = "pack")]
    if matches!(cli.command, Command::Pack { .. }) && cli.config.is_some() {
        return Err(VaireError::Usage(
            "`vaire pack` packs the package's committed knowledge.toml; --config is not supported"
                .into(),
        ));
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
            local,
        } => {
            let out = commands::search::run(
                &ctx,
                &query,
                type_filter.as_deref(),
                scope.as_deref(),
                Some(limit),
                local,
            )?;
            emit(&out, json);
        }
        Command::Suggest {
            descriptor,
            type_filter,
            limit,
            local,
        } => {
            let out = commands::suggest::run(
                &ctx,
                &descriptor,
                type_filter.as_deref(),
                Some(limit),
                local,
            )?;
            emit(&out, json);
        }
        Command::Unresolved {
            type_filter,
            scope,
            all_packages,
        } => {
            let out = commands::unresolved::run(
                &ctx,
                type_filter.as_deref(),
                scope.as_deref(),
                all_packages,
            )?;
            emit(&out, json);
        }
        Command::Index {
            full,
            working_tree,
            re_embed,
            no_deps,
        } => {
            emit(
                &commands::index::run(&ctx, full, working_tree, re_embed, no_deps)?,
                json,
            );
        }
        Command::Check {
            strict,
            working_tree,
            no_deps,
        } => {
            let (report, failed) = commands::check::run(&ctx, strict, working_tree, no_deps)?;
            emit(&report, json);
            if failed {
                return Ok(ExitCode::CheckViolations);
            }
        }
        Command::Status => {
            emit(&commands::status::run(&ctx)?, json);
        }
        #[cfg(feature = "pack")]
        Command::Pack { no_embeddings } => {
            emit(&commands::pack::run(&ctx, no_embeddings)?, json);
        }
        Command::Deps => {
            emit(&commands::deps::run(&ctx)?, json);
        }
        Command::Init { .. }
        | Command::Mcp
        | Command::Configure { .. }
        | Command::Add { .. }
        | Command::Upgrade { .. } => {
            unreachable!("handled above")
        }
    }

    Ok(ExitCode::Success)
}

/// Read a non-empty API key without ever placing it in argv. A trailing newline is accepted
/// for `printf ... | vaire configure embeddings --api-key-stdin` ergonomics.
fn read_api_key_from_stdin() -> Result<String> {
    const MAX_API_KEY_BYTES: u64 = 16 * 1024;

    let mut key = String::new();
    std::io::stdin()
        .take(MAX_API_KEY_BYTES + 1)
        .read_to_string(&mut key)?;
    if key.len() as u64 > MAX_API_KEY_BYTES {
        return Err(VaireError::Usage(
            "--api-key-stdin accepts at most 16 KiB".into(),
        ));
    }
    let key = key.trim_end_matches(['\r', '\n']);
    if key.is_empty() {
        return Err(VaireError::Usage(
            "--api-key-stdin requires a non-empty key on standard input".into(),
        ));
    }
    Ok(key.to_string())
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
