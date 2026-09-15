//! Does the index hold up when more than one vaire process wants it? (issue #50)
//!
//! Turso takes an exclusive lock when a database is **opened**, so a second process that
//! opened `.vaire/index.db` while another held it used to fail on the spot — and was told
//! the index was *corrupt* (exit `3`), the one diagnosis that invites rebuilding a perfectly
//! healthy file. Two `vaire resolve`s at once could collide; a read during `vaire index`
//! reliably did.
//!
//! Every index open now takes `.vaire/index.lock` first (`crate::db::DbLock`): an OS file
//! lock, so a process that finds it taken is parked by the kernel until the holder lets go,
//! rather than polling or failing. `vaire index` holds it for its whole run, which both
//! serializes two builds and gives a reader something to wait *for*.
//!
//! Like `catalog_concurrency.rs`, these tests spawn real OS processes. They have to: the
//! lock is per process, so threads in one test binary would share it and prove nothing. The
//! readers are the real `vaire` binary — that is also how the exit codes get exercised — and
//! the process holding the index is this test binary re-invoked as a worker.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::time::{Duration, Instant};

use common::{Corpus, DummyEmbedder};
use serde_json::{Value, json};
use vaire::index::build::{self, Mode};

/// Readers started at once. Above any plausible number of concurrent vaire invocations on
/// one machine, which is the point.
const READERS: usize = 12;

/// Full rebuilds the `build` worker runs back to back.
const BUILDS: usize = 3;

/// The environment variable that turns this test binary into a worker process. Absent in a
/// normal run, so the worker test below is a no-op unless a parent asked for it.
const WORKER_ENV: &str = "VAIRE_INDEX_WORKER";

/// The worker half: hold the index (or rebuild it) until the parent says stop. Invoked only
/// as a child process, with `<mode>|<target>|<signal dir>`.
#[test]
fn index_worker() {
    let Ok(spec) = std::env::var(WORKER_ENV) else {
        return;
    };
    let mut spec = spec.splitn(3, '|');
    let mode = spec.next().expect("worker mode");
    let target = PathBuf::from(spec.next().expect("worker target"));
    let signals = PathBuf::from(spec.next().expect("worker signal dir"));
    // The binary sets the wait policy from the environment at startup; a worker is its own
    // process, so it has to do the same.
    vaire::db::set_lock_wait(vaire::db::lock_wait_from_env(None).expect("lock timeout"));

    match mode {
        // Hold the index the way every vaire process does — through its lock.
        "hold" => {
            let _index = vaire::index::Index::open(&target).expect("worker opens the index");
            hold_until_released(&signals);
        }
        // Hold the *file* without taking vaire's lock: what an older vaire, or any other
        // tool that opens index.db directly, does.
        "hold-raw" => {
            let _db = vaire::db::Db::connect(&target).expect("worker opens the database");
            hold_until_released(&signals);
        }
        // Rebuild in a loop. The first rebuild starts under a lock the worker takes — and
        // reports — before building, so the parent can queue readers behind a rebuild that
        // has provably not started; later rebuilds each take the lock for their own run.
        "build" => {
            let repo = vaire::corpus::Repo::discover(Some(&target), &target).expect("repo");
            let config =
                vaire::config::Config::load(&target.join("knowledge.toml")).expect("manifest");
            // A corpus that has never been built has no derived directory yet.
            std::fs::create_dir_all(target.join(".vaire")).expect("derived dir");
            let mut first = Some(
                vaire::db::DbLock::acquire(&target.join(".vaire/index.lock"), "the index")
                    .expect("worker takes the index lock"),
            );
            hold_until_released(&signals);
            for _ in 0..BUILDS {
                build::run(&repo, &config, &DummyEmbedder { dims: 8 }, Mode::Full)
                    .expect("worker rebuilds the index");
                drop(first.take());
            }
        }
        other => panic!("unknown worker mode: {other}"),
    }
}

/// Many readers queued behind one holder, then let go together: every one of them answers.
///
/// The holder is the barrier. Every reader has announced that it is waiting before the holder
/// lets go, so they provably contend — with it, and then with each other — rather than merely
/// having been started around the same time.
#[test]
fn concurrent_reads_all_succeed() {
    let c = Corpus::fixture();
    let signals = tempfile::tempdir().expect("tempdir");
    let mut holder = spawn_worker("hold", &c.repo().index_db(), signals.path(), None);
    wait_until_ready(&mut holder, signals.path());

    let stderr = stderr_files(signals.path(), "reader", READERS);
    let mut readers: Vec<Child> = stderr
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let args: &[&str] = match i % 2 {
                0 => &["resolve", "person:jane-doe"],
                _ => &["backlinks", "person:jane-doe"],
            };
            spawn_reader(&c, args, path)
        })
        .collect();
    wait_until_waiting(&mut readers, &stderr);

    let started = Instant::now();
    release(signals.path());
    let failures = failed_readers(readers, &stderr);

    assert!(failures.is_empty(), "readers failed: {failures:?}");
    assert!(finish(holder, Duration::from_secs(30)).status.success());
    println!(
        "index concurrency: {READERS} queued reads answered in {:?}",
        started.elapsed()
    );
}

/// A read started while another process has the index open waits for it, and then answers.
///
/// The waiting is the whole fix: before it, this read failed immediately, claiming the index
/// was corrupt.
#[test]
fn a_read_waits_for_the_holder_and_then_succeeds() {
    let c = Corpus::fixture();
    let signals = tempfile::tempdir().expect("tempdir");
    let mut holder = spawn_worker("hold", &c.repo().index_db(), signals.path(), None);
    wait_until_ready(&mut holder, signals.path());

    // Waited for on the notice itself, not on a sleep: how soon a freshly spawned process
    // reaches the lock is up to the machine. By the time the notice is out, the old code
    // would long since have exited claiming corruption — the reader must still be here.
    let stderr = stderr_files(signals.path(), "reader", 1);
    let mut readers = vec![spawn_reader(
        &c,
        &["resolve", "person:jane-doe"],
        &stderr[0],
    )];
    wait_until_waiting(&mut readers, &stderr);
    assert!(
        readers[0].try_wait().expect("try_wait").is_none(),
        "the reader is still waiting after announcing it"
    );

    release(signals.path());
    let failures = failed_readers(readers, &stderr);
    assert!(
        failures.is_empty(),
        "the read succeeds once the index is free: {failures:?}"
    );
    assert!(
        finish(holder, Duration::from_secs(30)).status.success(),
        "the holder exits cleanly"
    );
}

/// Reads queued behind a rebuild, then more reads while further rebuilds run: none fails, and
/// none is told to rebuild a healthy index.
///
/// The builder takes the index lock and reports before it builds, and does not start until
/// every queued reader has announced that it is waiting — so the first rebuild provably
/// overlaps them.
#[test]
fn reads_during_rebuilds_succeed() {
    const QUEUED: usize = 4;
    let c = Corpus::fixture();
    let signals = tempfile::tempdir().expect("tempdir");
    let mut builder = spawn_worker("build", c.root(), signals.path(), None);
    wait_until_ready(&mut builder, signals.path());

    let stderr = stderr_files(signals.path(), "queued", QUEUED);
    let mut queued: Vec<Child> = stderr
        .iter()
        .map(|path| spawn_reader(&c, &["resolve", "person:jane-doe"], path))
        .collect();
    wait_until_waiting(&mut queued, &stderr);
    release(signals.path()); // the first rebuild starts, with every queued reader behind it

    let mut failures = failed_readers(queued, &stderr);
    let mut reads = QUEUED;
    // And reads started while the remaining rebuilds run.
    loop {
        let reader = vaire(&c, &["resolve", "person:jane-doe"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn reader");
        let out = finish(reader, Duration::from_secs(120));
        reads += 1;
        if !out.status.success() {
            failures.push(format!(
                "{} — {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        if builder.try_wait().expect("try_wait").is_some() {
            break;
        }
    }

    assert!(
        finish(builder, Duration::from_secs(120)).status.success(),
        "the rebuilding worker exits cleanly"
    );
    assert!(
        failures.is_empty(),
        "reads during a rebuild failed: {failures:?}"
    );
    println!("index concurrency: {reads} reads across {BUILDS} full rebuilds");
}

/// A read during the very first build waits for it, rather than reporting "not built".
///
/// A first build stages its index and only creates `index.db` when it promotes it at the end,
/// so a read that looked for the file before taking the lock would exit `4` for an index that
/// was seconds from existing.
#[test]
fn a_read_during_the_first_build_waits_for_it() {
    let c = Corpus::empty();
    c.add(
        "knowledge/jane.md",
        "---\nid: jane-doe\ntype: person\nname: Jane Doe\n---\n# Jane Doe\n",
    )
    .commit();
    assert!(
        !c.repo().index_db().exists(),
        "nothing has built this corpus yet"
    );
    let signals = tempfile::tempdir().expect("tempdir");
    let mut builder = spawn_worker("build", c.root(), signals.path(), None);
    wait_until_ready(&mut builder, signals.path());

    let stderr = stderr_files(signals.path(), "first", 1);
    let mut readers = vec![spawn_reader(
        &c,
        &["resolve", "person:jane-doe"],
        &stderr[0],
    )];
    wait_until_waiting(&mut readers, &stderr);
    release(signals.path()); // the first build starts, with the reader behind it

    let failures = failed_readers(readers, &stderr);
    assert!(
        failures.is_empty(),
        "the read answers from the first build: {failures:?}"
    );
    assert!(finish(builder, Duration::from_secs(120)).status.success());
}

/// Giving up on a held index is a lock error, never exit `3` — which would send whoever read
/// it off to rebuild a healthy index.
#[test]
fn a_bounded_wait_ends_in_index_locked_not_corrupt() {
    let c = Corpus::fixture();
    let signals = tempfile::tempdir().expect("tempdir");
    let mut holder = spawn_worker("hold", &c.repo().index_db(), signals.path(), None);
    wait_until_ready(&mut holder, signals.path());

    let reader = vaire(&c, &["-o", "json", "resolve", "person:jane-doe"])
        .env("VAIRE_LOCK_TIMEOUT", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn reader");
    let out = finish(reader, Duration::from_secs(60));

    assert_eq!(
        out.status.code(),
        Some(1),
        "a locked index is exit 1, not exit 3 (corrupt): {}",
        String::from_utf8_lossy(&out.stdout).trim()
    );
    let error: Value = serde_json::from_slice(&out.stdout).expect("json error value");
    assert_eq!(error["error"]["kind"], "index_locked");
    assert_eq!(error["error"]["code"], 1);

    release(signals.path());
    assert!(finish(holder, Duration::from_secs(30)).status.success());
}

/// A process that opens the file *without* vaire's lock — an older vaire, another tool — is
/// reported as holding it, rather than as having corrupted it.
#[test]
fn an_index_held_outside_the_lock_is_locked_not_corrupt() {
    let c = Corpus::fixture();
    let signals = tempfile::tempdir().expect("tempdir");
    let mut holder = spawn_worker("hold-raw", &c.repo().index_db(), signals.path(), None);
    wait_until_ready(&mut holder, signals.path());

    let reader = vaire(&c, &["-o", "json", "resolve", "person:jane-doe"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn reader");
    let out = finish(reader, Duration::from_secs(60));

    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stdout).expect("json error value");
    assert_eq!(error["error"]["kind"], "index_locked");

    release(signals.path());
    assert!(finish(holder, Duration::from_secs(30)).status.success());
}

/// A resident MCP server closes each index before answering, so an agent session that stays
/// open for hours never locks `vaire index` out of the package it read.
#[test]
fn mcp_releases_the_index_between_requests() {
    let c = Corpus::fixture();
    let mut server = vaire(&c, &["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn vaire mcp");
    let mut stdin = server.stdin.take().expect("server stdin");
    let mut stdout = BufReader::new(server.stdout.take().expect("server stdout"));

    let result = call_tool(&mut stdin, &mut stdout, 1, "resolve", "person:jane-doe");
    assert_eq!(
        result["isError"], false,
        "the server answers the first call"
    );

    // The server is still running, having just read the index. Another process must be able
    // to take it — bounded, so a server that kept it fails this test instead of hanging it.
    let signals = tempfile::tempdir().expect("tempdir");
    let mut holder = spawn_worker(
        "hold",
        &c.repo().index_db(),
        signals.path(),
        Some(("VAIRE_LOCK_TIMEOUT", "10")),
    );
    wait_until_ready(&mut holder, signals.path());
    release(signals.path());
    assert!(
        finish(holder, Duration::from_secs(30)).status.success(),
        "a running MCP server does not hold the index between requests"
    );

    drop(stdin); // EOF → the server exits
    assert!(finish(server, Duration::from_secs(30)).status.success());
}

/// A tool call against an index someone else holds gives up with an `index_locked` tool
/// error rather than leaving the agent hanging behind a long rebuild.
#[test]
fn an_mcp_call_gives_up_with_index_locked() {
    let c = Corpus::fixture();
    let signals = tempfile::tempdir().expect("tempdir");
    let mut holder = spawn_worker("hold", &c.repo().index_db(), signals.path(), None);
    wait_until_ready(&mut holder, signals.path());

    let mut server = vaire(&c, &["mcp"])
        .env("VAIRE_LOCK_TIMEOUT", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn vaire mcp");
    let mut stdin = server.stdin.take().expect("server stdin");
    let mut stdout = BufReader::new(server.stdout.take().expect("server stdout"));

    let result = call_tool(&mut stdin, &mut stdout, 1, "resolve", "person:jane-doe");
    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().expect("tool text");
    let error: Value = serde_json::from_str(text).expect("json error value");
    assert_eq!(error["error"]["kind"], "index_locked");
    assert_eq!(error["error"]["code"], 1, "not exit 3 — the index is fine");

    release(signals.path());
    assert!(finish(holder, Duration::from_secs(30)).status.success());
    drop(stdin);
    assert!(finish(server, Duration::from_secs(30)).status.success());
}

// ---- the parent's half ------------------------------------------------------------------

/// What a `vaire` process prints once it has been waiting for a lock for a second.
const WAIT_NOTICE: &str = "waiting for another vaire process";

/// One stderr file per reader, so a test can watch readers while they run.
fn stderr_files(dir: &Path, prefix: &str, count: usize) -> Vec<PathBuf> {
    (0..count)
        .map(|i| dir.join(format!("{prefix}-{i}.stderr")))
        .collect()
}

/// Start a `vaire` reader with its stderr going to `stderr`.
fn spawn_reader(c: &Corpus, args: &[&str], stderr: &Path) -> Child {
    vaire(c, args)
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(stderr).expect("reader stderr file"))
        .spawn()
        .expect("spawn reader")
}

/// The barrier: block until every reader has announced that it is waiting for the lock.
///
/// Proof that each one really is queued behind the holder, rather than merely started — and
/// a reader that exits before announcing fails the test, since with the lock held it could
/// only have exited by failing.
fn wait_until_waiting(readers: &mut [Child], stderr: &[PathBuf]) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let mut all_waiting = true;
        for (reader, path) in readers.iter_mut().zip(stderr) {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            if text.contains(WAIT_NOTICE) {
                continue;
            }
            all_waiting = false;
            if let Some(status) = reader.try_wait().expect("try_wait") {
                panic!("a reader exited instead of waiting for the held index ({status}): {text}");
            }
        }
        if all_waiting {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "not every reader announced that it was waiting within 60s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Wait for every reader, returning a description of each one that did not succeed.
fn failed_readers(readers: Vec<Child>, stderr: &[PathBuf]) -> Vec<String> {
    readers
        .into_iter()
        .zip(stderr)
        .filter_map(|(reader, path)| {
            let out = finish(reader, Duration::from_secs(120));
            (!out.status.success()).then(|| {
                format!(
                    "{}: {} — {}",
                    path.display(),
                    out.status,
                    std::fs::read_to_string(path).unwrap_or_default().trim()
                )
            })
        })
        .collect()
}

/// A `vaire` invocation against `c`, with its own hermetic home.
fn vaire(c: &Corpus, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_vaire"));
    cmd.arg("--repo")
        .arg(c.root())
        .args(args)
        .env("VAIRE_HOME", c.root().join(".vaire-home"))
        // Whatever the developer running the suite has set must not decide what these
        // assert: each test states the policy it is testing.
        .env_remove("VAIRE_LOCK_TIMEOUT")
        .env_remove("VAIRE_OUTPUT");
    cmd
}

/// Re-invoke this test binary as a worker in `mode`, coordinating through `signals`.
fn spawn_worker(mode: &str, target: &Path, signals: &Path, env: Option<(&str, &str)>) -> Child {
    let mut cmd = Command::new(std::env::current_exe().expect("test binary path"));
    cmd.args(["index_worker", "--exact", "--quiet"])
        .env(
            WORKER_ENV,
            format!("{mode}|{}|{}", target.display(), signals.display()),
        )
        .env_remove("VAIRE_LOCK_TIMEOUT")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some((key, value)) = env {
        cmd.env(key, value);
    }
    cmd.spawn().expect("spawn worker")
}

/// Block until the parent drops the `release` marker (or long enough that a forgotten worker
/// cannot outlive the suite).
fn hold_until_released(signals: &Path) {
    std::fs::write(signals.join("ready"), b"").expect("ready marker");
    let deadline = Instant::now() + Duration::from_secs(120);
    while !signals.join("release").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Wait until the worker reports that it has the index.
fn wait_until_ready(worker: &mut Child, signals: &Path) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !signals.join("ready").exists() {
        if let Some(status) = worker.try_wait().expect("try_wait") {
            panic!("the worker exited before it took the index: {status}");
        }
        assert!(Instant::now() < deadline, "the worker never took the index");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Tell a worker blocked in [`hold_until_released`] to let go.
fn release(signals: &Path) {
    std::fs::write(signals.join("release"), b"").expect("release marker");
}

/// Wait for a process, failing the test rather than the suite if it never finishes.
fn finish(mut child: Child, limit: Duration) -> Output {
    let deadline = Instant::now() + limit;
    while child.try_wait().expect("try_wait").is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("a process did not finish within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().expect("process output")
}

/// One `tools/call` over the MCP wire, returning its result object.
fn call_tool(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    id: i64,
    tool: &str,
    node: &str,
) -> Value {
    let request = json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": tool, "arguments": { "id": node } }
    });
    writeln!(stdin, "{request}").expect("write request");
    stdin.flush().expect("flush request");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    let response: Value = serde_json::from_str(&line).expect("JSON-RPC response");
    response["result"].clone()
}
