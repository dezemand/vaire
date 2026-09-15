//! `cargo bench --bench search -- [flags]` — the search quality + latency benchmark for
//! issue #52 (`vaire search` ranks long spec documents above short, more relevant
//! concept/principle nodes). See `benches/search/README.md` for the full contract: corpora,
//! query-file format, metrics, and how to compare two branches.
//!
//! Custom harness (`harness = false` in Cargo.toml) with hand-rolled argument parsing —
//! `cargo bench` always appends `--bench` to a bench binary's own args, harness or not, so
//! that token is explicitly ignored below.

#[path = "eval/mod.rs"]
mod eval;

use std::path::PathBuf;
use std::process::Command;

use eval::embedders::{EmbedderHandle, EmbedderKind};

struct Cli {
    corpora: Vec<String>,
    external_dir: Option<PathBuf>,
    external_queries: Option<PathBuf>,
    external_name: String,
    embedder: EmbedderKind,
    vector_cache: Option<PathBuf>,
    out_dir: PathBuf,
    label: Option<String>,
    queries_override: Option<PathBuf>,
    repeat: usize,
    scale_nodes: usize,
    verbose: bool,
    compare: Option<(PathBuf, PathBuf)>,
}

const HELP: &str = "\
cargo bench --bench search -- [flags]

  --corpus <list>          public|external|scale, comma-separated, or `all` (default: public)
  --external-dir <dir>     your own corpus root (env VAIRE_BENCH_EXTERNAL_DIR)
  --external-queries <f>   your own judgments file (env VAIRE_BENCH_EXTERNAL_QUERIES)
  --external-name <label>  label for the external corpus in reports (default: external)
  --embedder <kind>        local|cached|openai (default: local)
  --vector-cache <file>    required for --embedder cached|openai
  --queries <file>         override queries for a single selected corpus (smoke tests)
  --repeat <N>             timed repeats per query (default: 5)
  --scale-nodes <N>        node count for the scale corpus (default: 3000)
  --out <dir>              JSON report directory (default: target/search-bench)
  --label <string>         report label (default: <branch>-<shortsha>, else unlabeled)
  --verbose                print the per-query Markdown table too
  --compare <base> <new>   print base -> new deltas from two JSON reports; builds nothing
  -h, --help               print this message
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = match parse_args(args) {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("error: {e}\n\n{HELP}");
            std::process::exit(2);
        }
    };

    if let Some((base, new)) = &cli.compare {
        match run_compare(base, new) {
            Ok(md) => println!("{md}"),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if let Err(e) = run(&cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn parse_args(args: Vec<String>) -> Result<Cli, String> {
    let mut corpora: Option<Vec<String>> = None;
    let mut external_dir = None;
    let mut external_queries = None;
    let mut external_name = "external".to_string();
    let mut embedder = EmbedderKind::Local;
    let mut vector_cache = None;
    let mut out_dir = PathBuf::from("target/search-bench");
    let mut label = None;
    let mut queries_override = None;
    let mut repeat = 5usize;
    let mut scale_nodes = 3000usize;
    let mut verbose = false;
    let mut compare = None;

    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            // `cargo bench` appends this to every bench binary's args, custom harness or
            // not — it is not a flag of ours.
            "--bench" => {}
            "--corpus" => corpora = Some(parse_corpus_list(&next_val(&mut it, "--corpus")?)?),
            "--external-dir" => {
                external_dir = Some(PathBuf::from(next_val(&mut it, "--external-dir")?))
            }
            "--external-queries" => {
                external_queries = Some(PathBuf::from(next_val(&mut it, "--external-queries")?))
            }
            "--external-name" => external_name = next_val(&mut it, "--external-name")?,
            "--embedder" => embedder = EmbedderKind::parse(&next_val(&mut it, "--embedder")?)?,
            "--vector-cache" => {
                vector_cache = Some(PathBuf::from(next_val(&mut it, "--vector-cache")?))
            }
            "--out" => out_dir = PathBuf::from(next_val(&mut it, "--out")?),
            "--label" => label = Some(next_val(&mut it, "--label")?),
            "--queries" => queries_override = Some(PathBuf::from(next_val(&mut it, "--queries")?)),
            "--repeat" => {
                repeat = next_val(&mut it, "--repeat")?
                    .parse()
                    .map_err(|_| "--repeat must be a positive integer".to_string())?;
            }
            "--scale-nodes" => {
                scale_nodes = next_val(&mut it, "--scale-nodes")?
                    .parse()
                    .map_err(|_| "--scale-nodes must be a positive integer".to_string())?;
            }
            "--verbose" => verbose = true,
            "--compare" => {
                let base = next_val(&mut it, "--compare")?;
                let new = next_val(&mut it, "--compare")?;
                compare = Some((PathBuf::from(base), PathBuf::from(new)));
            }
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            other => return Err(format!("unrecognized argument: {other}")),
        }
    }

    if external_dir.is_none() {
        external_dir = std::env::var("VAIRE_BENCH_EXTERNAL_DIR")
            .ok()
            .map(PathBuf::from);
    }
    if external_queries.is_none() {
        external_queries = std::env::var("VAIRE_BENCH_EXTERNAL_QUERIES")
            .ok()
            .map(PathBuf::from);
    }

    if vector_cache.is_none() && matches!(embedder, EmbedderKind::Cached | EmbedderKind::OpenAi) {
        return Err(format!(
            "--embedder {} requires --vector-cache <file>",
            embedder.as_str()
        ));
    }

    let corpora = corpora.unwrap_or_else(|| vec!["public".to_string()]);
    if queries_override.is_some() && corpora.len() != 1 {
        return Err(
            "--queries overrides a single corpus's queries — select exactly one --corpus"
                .to_string(),
        );
    }

    Ok(Cli {
        corpora,
        external_dir,
        external_queries,
        external_name,
        embedder,
        vector_cache,
        out_dir,
        label,
        queries_override,
        repeat: repeat.max(1),
        scale_nodes: scale_nodes.max(1),
        verbose,
        compare,
    })
}

fn next_val(it: &mut std::vec::IntoIter<String>, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_corpus_list(v: &str) -> Result<Vec<String>, String> {
    if v == "all" {
        // `external` is opportunistically included by the run loop only when both its
        // inputs are actually available; listing it here just means "try it".
        return Ok(vec![
            "public".to_string(),
            "scale".to_string(),
            "external".to_string(),
        ]);
    }
    let mut out = Vec::new();
    for part in v.split(',') {
        match part.trim() {
            "public" | "external" | "scale" => out.push(part.trim().to_string()),
            other => {
                return Err(format!(
                    "unknown corpus {other:?} (expected public|external|scale|all)"
                ));
            }
        }
    }
    if out.is_empty() {
        return Err("--corpus requires at least one value".to_string());
    }
    Ok(out)
}

fn run_compare(base_path: &std::path::Path, new_path: &std::path::Path) -> Result<String, String> {
    let base = eval::report::read_json(base_path)?;
    let new = eval::report::read_json(new_path)?;
    Ok(eval::report::render_compare_markdown(&base, &new))
}

fn run(cli: &Cli) -> Result<(), String> {
    eval::embedders::hermetic_config_home_unless_openai(cli.embedder)
        .map_err(|e| format!("setting up a hermetic config home: {e}"))?;

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let embedder = match cli.embedder {
        EmbedderKind::Local => EmbedderHandle::local()?,
        EmbedderKind::Cached => EmbedderHandle::cached(
            cli.vector_cache
                .as_deref()
                .expect("validated by parse_args"),
        )?,
        EmbedderKind::OpenAi => EmbedderHandle::openai(
            cli.vector_cache
                .as_deref()
                .expect("validated by parse_args"),
        )?,
    };

    let (git_branch, git_sha) = git_branch_and_sha(&repo_root);
    let label = cli
        .label
        .clone()
        .unwrap_or_else(|| match (&git_branch, &git_sha) {
            // Branch names routinely contain `/` (`feat/x`, `fix/y`); the default label also
            // becomes a JSON filename (`eval::report::write_json`), so keep it flat.
            (Some(b), Some(s)) => format!("{}-{s}", b.replace('/', "-")),
            _ => "unlabeled".to_string(),
        });

    let mut run_report = eval::report::RunReport {
        label,
        git_branch,
        git_sha,
        embedder_identity: embedder.as_dyn().identity(),
        generated_at_unix_ms: now_unix_ms(),
        corpora: Vec::new(),
    };

    for name in &cli.corpora {
        match name.as_str() {
            "public" => {
                let build = eval::corpus::build_public(&repo_root, None)?;
                let default_queries = repo_root.join("benches/search/data/public/queries.toml");
                let queries_path = cli.queries_override.clone().unwrap_or(default_queries);
                let (queries, notices) =
                    load_queries_gracefully(&queries_path, cli.queries_override.is_some())?;
                let notices = [build.notices.clone(), notices].concat();
                let report = eval::runner::run_corpus(
                    "public",
                    &build,
                    embedder.as_dyn(),
                    &queries,
                    notices,
                    cli.repeat,
                )?;
                run_report.corpora.push(report);
            }
            "external" => {
                let (Some(dir), Some(queries_path)) = (&cli.external_dir, &cli.external_queries)
                else {
                    println!(
                        "skip: external corpus (set --external-dir/--external-queries or \
                         VAIRE_BENCH_EXTERNAL_DIR/VAIRE_BENCH_EXTERNAL_QUERIES to include it)"
                    );
                    continue;
                };
                let queries_path = cli
                    .queries_override
                    .clone()
                    .unwrap_or_else(|| queries_path.clone());
                let build = eval::corpus::build_external(dir)?;
                let (queries, notices) =
                    load_queries_gracefully(&queries_path, cli.queries_override.is_some())?;
                let report = eval::runner::run_corpus(
                    &cli.external_name,
                    &build,
                    embedder.as_dyn(),
                    &queries,
                    notices,
                    cli.repeat,
                )?;
                run_report.corpora.push(report);
            }
            "scale" => {
                let (build, generated_queries) =
                    eval::corpus::build_scale(cli.scale_nodes, eval::corpus::SCALE_SEED)?;
                let queries = match &cli.queries_override {
                    Some(path) => load_queries_gracefully(path, true)?.0,
                    None => generated_queries,
                };
                let report = eval::runner::run_corpus(
                    "scale",
                    &build,
                    embedder.as_dyn(),
                    &queries,
                    build.notices.clone(),
                    cli.repeat,
                )?;
                run_report.corpora.push(report);
            }
            other => return Err(format!("unknown corpus {other:?}")),
        }
    }

    embedder.flush()?;

    let markdown = eval::report::render_markdown(&run_report, cli.verbose);
    println!("{markdown}");

    let json_path = eval::report::write_json(&run_report, &cli.out_dir)?;
    eprintln!("wrote {}", json_path.display());

    Ok(())
}

/// Load a query file, or skip gracefully (no error) if it's the corpus's own default path
/// that simply doesn't exist yet. An explicit `--queries` override that doesn't exist IS an
/// error — the whole point of pointing at it was to use that exact file.
fn load_queries_gracefully(
    path: &std::path::Path,
    is_override: bool,
) -> Result<(Vec<eval::queries::Query>, Vec<String>), String> {
    if !path.is_file() {
        if is_override {
            return Err(format!("--queries {} not found", path.display()));
        }
        return Ok((
            Vec::new(),
            vec![format!(
                "no queries file at {} yet — building the corpus but skipping quality metrics \
                 (0 queries; the latency table will be empty)",
                path.display()
            )],
        ));
    }
    let queries = eval::queries::load(path)?;
    Ok((queries, Vec::new()))
}

fn now_unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// `(branch, short sha)` via the system `git`, run against `repo_root` — deliberately not
/// `vaire::git` (this harness sticks to `vaire`'s documented public surface, see the
/// README). `None`/`None` if `git` is unavailable or `repo_root` isn't a repo.
fn git_branch_and_sha(repo_root: &std::path::Path) -> (Option<String>, Option<String>) {
    let branch = git_output(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let sha = git_output(repo_root, &["rev-parse", "--short", "HEAD"]);
    (branch, sha)
}

fn git_output(repo_root: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}
