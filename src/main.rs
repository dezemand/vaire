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
    if let Command::Add {
        spec,
        link,
        no_register,
    } = &cli.command
    {
        let out = commands::add::run(
            cli.repo.as_deref(),
            cli.config.as_deref(),
            spec,
            link.as_deref(),
        )?;
        // Declaring a dependency is a statement that this package exists and is being
        // worked on, so it is worth recording — the manifest's directory is the package.
        if let Some(root) = std::path::Path::new(&out.config_path).parent() {
            for warning in commands::catalog::register_path(
                &vaire::userconfig::vaire_home(),
                root,
                *no_register,
            ) {
                eprintln!("warning: {warning}");
            }
        }
        emit(&out, json);
        return Ok(ExitCode::Success);
    }

    // The catalog is machine-level state *about* packages, so it has to work from
    // anywhere — notably from outside any package, which is exactly where
    // `catalog add <path>` and `catalog scan <dir>` are used. It never builds a `Ctx`.
    if let Command::Catalog { action } = cli.command {
        let home = vaire::userconfig::vaire_home();
        use vaire::cli::CatalogAction;
        match action {
            CatalogAction::Add { path } => {
                emit(&commands::catalog::add(&home, path.as_deref())?, json)
            }
            CatalogAction::Scan { dir } => emit(&commands::catalog::scan_dir(&home, &dir)?, json),
            CatalogAction::Rm { target, missing } => emit(
                &commands::catalog::remove(&home, target.as_deref(), missing)?,
                json,
            ),
            CatalogAction::List => emit(&commands::catalog::list(&home)?, json),
        }
        return Ok(ExitCode::Success);
    }

    // Registries are machine-level configuration, like the catalog: usable from anywhere,
    // and never needing a package to stand in.
    if let Command::Registry { action } = cli.command {
        let home = vaire::userconfig::vaire_home();
        use vaire::cli::RegistryAction;
        match action {
            RegistryAction::Add {
                name,
                url,
                priority,
                no_search,
            } => emit(
                &commands::registry::add(&home, &name, &url, priority, !no_search)?,
                json,
            ),
            RegistryAction::List => emit(&commands::registry::list(&home)?, json),
            RegistryAction::Rm { name } => emit(&commands::registry::remove(&home, &name)?, json),
            RegistryAction::Show { name } => emit(&commands::registry::show(&home, &name)?, json),
        }
        return Ok(ExitCode::Success);
    }

    // `yank` acts on a registry, not on a corpus: after a bad publish, the package whose
    // release is being withdrawn is often not the directory you are standing in.
    if let Command::Yank {
        spec,
        registry,
        undo,
    } = &cli.command
    {
        let home = vaire::userconfig::vaire_home();
        let out = commands::yank::run(&home, spec, registry.as_deref(), *undo)?;
        emit(&out, json);
        return Ok(ExitCode::Success);
    }

    // `mcp` builds its own long-lived context and never returns output. Outside any
    // package it serves the rootless session, which is the point of exposing it that way:
    // an agent can be pointed at the machine rather than at one checkout.
    if let Command::Mcp = cli.command {
        let ctx = read_ctx(cli.repo, cli.config, false, cli.frozen)?.with_frozen(cli.frozen);
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

    // `pull <name>` is a store operation, not a corpus one: "put this package on this
    // machine" means the same thing from anywhere, and requiring a package to stand in
    // would make the obvious first command fail on a fresh machine. Bare `vaire pull`
    // stays package-scoped, because it works from the manifest's dependencies.
    #[cfg(feature = "pack")]
    if let Command::Pull {
        spec: Some(spec),
        registry,
        // `--locked` is refused for a named pull anyway (it reproduces a whole recorded
        // resolution), so it rides along only to be reported by the command's own check
        // rather than swallowed by this pattern.
        locked,
        dry_run,
    } = &cli.command
        && Ctx::new(cli.repo.clone(), cli.config.clone()).is_err()
    {
        let ctx = Ctx::rootless(vaire::userconfig::vaire_home())?;
        let out = commands::pull::run(
            &ctx,
            commands::pull::Options {
                spec: Some(spec),
                registry: registry.as_deref(),
                locked: *locked,
                dry_run: *dry_run,
            },
        )?;
        let failed = !out.failed.is_empty();
        emit(&out, json);
        return Ok(match failed {
            true => ExitCode::Generic,
            false => ExitCode::Success,
        });
    }

    // Read commands fall back to the catalog when there is no package to stand in;
    // maintain commands keep erroring, because there is nothing for them to maintain.
    let ctx = match cli.command.is_read() {
        true => read_ctx(cli.repo, cli.config, cli.command.wants_all(), cli.frozen)?,
        false => Ctx::new(cli.repo, cli.config)?,
    }
    .with_frozen(cli.frozen);

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
            all: _,
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
            all: _,
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
            no_register,
        } => {
            let out = commands::index::run(&ctx, full, working_tree, re_embed, no_deps)?;
            for warning in commands::catalog::register_ambient(&ctx, no_register) {
                eprintln!("warning: {warning}");
            }
            emit(&out, json);
        }
        Command::Check {
            strict,
            working_tree,
            no_deps,
            no_register,
        } => {
            let (report, failed) = commands::check::run(&ctx, strict, working_tree, no_deps)?;
            for warning in commands::catalog::register_ambient(&ctx, no_register) {
                eprintln!("warning: {warning}");
            }
            emit(&report, json);
            if failed {
                return Ok(ExitCode::CheckViolations);
            }
        }
        Command::Status => {
            emit(&commands::status::run(&ctx)?, json);
        }
        Command::Release {
            major,
            dry_run,
            yes,
            notes,
            allow_branch,
            push,
            onto,
        } => {
            // Reserved grammar: the flag exists so the split it belongs to has somewhere
            // to grow, and is rejected rather than silently ignored.
            if onto.is_some() {
                return Err(VaireError::Usage(
                    "`--onto` (back-patching an older major line) is reserved, not yet \
                     implemented"
                        .into(),
                ));
            }
            let out = commands::release::run(
                &ctx,
                commands::release::Options {
                    major,
                    dry_run,
                    yes,
                    notes: notes.as_deref(),
                    allow_branch,
                },
            )?;
            let blocked = out.status == vaire::output::ReleaseStatus::Blocked;
            let released = out.status == vaire::output::ReleaseStatus::Released;
            emit(&out, json);
            if blocked {
                // Not a failure: a decision waiting on a maintainer. Its own code so a
                // pipeline can report it as pending rather than broken.
                return Ok(ExitCode::ReleaseBlocked);
            }
            // `--push` is a convenience over the split, never a merge of it: the release
            // has already been committed and tagged by the time this runs, so a failed
            // upload leaves a perfectly good tag that `vaire push` will publish on its own.
            // Nothing to publish (a dry run, or a no-op release) means nothing to do.
            #[cfg(feature = "pack")]
            if push && released {
                let out = commands::push::run(
                    &ctx,
                    commands::push::Options {
                        version: None,
                        registry: None,
                        access: None,
                        access_hint: None,
                        dry_run: false,
                    },
                )?;
                let failed = !out.failed.is_empty();
                emit(&out, json);
                if failed {
                    return Ok(ExitCode::Generic);
                }
            }
            #[cfg(not(feature = "pack"))]
            // Not gated on `released`, unlike the `pack` build above: there the guard means
            // "nothing was cut, so there is nothing to upload", and here the flag can never
            // do anything at all. Accepting it silently on a dry run would teach nothing.
            if push {
                return Err(VaireError::Usage(
                    "`--push` needs the `pack` feature: an artifact has to exist before it \
                     can be uploaded"
                        .into(),
                ));
            }
        }
        #[cfg(feature = "pack")]
        Command::Pack { no_embeddings } => {
            emit(&commands::pack::run(&ctx, no_embeddings)?, json);
        }
        #[cfg(feature = "pack")]
        Command::Pull {
            spec,
            registry,
            locked,
            dry_run,
        } => {
            let out = commands::pull::run(
                &ctx,
                commands::pull::Options {
                    spec: spec.as_deref(),
                    registry: registry.as_deref(),
                    locked,
                    dry_run,
                },
            )?;
            let failed = !out.failed.is_empty();
            emit(&out, json);
            if failed {
                return Ok(ExitCode::Generic);
            }
        }
        #[cfg(feature = "pack")]
        Command::Push {
            version,
            registry,
            access,
            access_hint,
            dry_run,
        } => {
            let out = commands::push::run(
                &ctx,
                commands::push::Options {
                    version: version.as_deref(),
                    registry: registry.as_deref(),
                    access: access.as_deref(),
                    access_hint: access_hint.as_deref(),
                    dry_run,
                },
            )?;
            let failed = !out.failed.is_empty();
            emit(&out, json);
            if failed {
                // Some versions did not publish. Reported per version above; the exit code
                // is what a pipeline branches on.
                return Ok(ExitCode::Generic);
            }
        }
        Command::Deps => {
            emit(&commands::deps::run(&ctx)?, json);
        }
        Command::Init { .. }
        | Command::Catalog { .. }
        | Command::Registry { .. }
        | Command::Yank { .. }
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

/// The context a **read** command runs in.
///
/// Ordinarily the package you are standing in. Outside one — or with `--all` — the
/// rootless session, scoped by the catalog (registry.v2.md §9). The fallback is confined
/// to reads on purpose: a maintain command has nothing to maintain without a package, and
/// its `no corpus found` error is the right answer rather than a scope substitution.
///
/// Note which way the fallback runs. It never rescues a *resolution* — an author's
/// declared dependency that cannot be located still fails, because a manifest that
/// silently resolves from ambient machine state has stopped meaning anything. It rescues
/// only the case where there is no manifest at all to betray.
fn read_ctx(
    repo: Option<std::path::PathBuf>,
    config: Option<std::path::PathBuf>,
    all: bool,
    frozen: bool,
) -> Result<Ctx> {
    let home = vaire::userconfig::vaire_home();
    // `--all` widens the closure query, so whatever package you are standing in stays in
    // scope — including one the catalog has never been told about. Standing nowhere is not
    // an error here: `--all` is exactly as valid from `~` as from inside a package.
    if all {
        let standing_in = Ctx::new(repo, config)
            .ok()
            .map(|ctx| (ctx.config.name.clone(), ctx.repo.root().to_path_buf()));
        return Ctx::rootless_with(home, standing_in, frozen);
    }
    // An explicit `--repo` / `VAIRE_REPO` naming something that is not a package is a
    // mistake to report, not the ambient "no package anywhere above me" the fallback is
    // for. Both arrive as `NoRepo`; only the ambient one may be answered with a different
    // scope, or a typo'd override would silently return results from the whole machine.
    let overridden = repo.is_some() || std::env::var_os("VAIRE_REPO").is_some();
    match Ctx::new(repo, config) {
        Err(VaireError::NoRepo) if !overridden => Ctx::rootless_with(home, None, frozen),
        other => other,
    }
}
