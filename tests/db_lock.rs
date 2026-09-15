//! The in-process half of `vaire::db::DbLock`: callers in one process share an acquisition,
//! and callers that give up do not each leave a thread behind (issue #50, review of #53).
//!
//! The contention still has to come from another process — an OS file lock belongs to a
//! process, so one process cannot contend with itself — so a worker (this test binary,
//! re-invoked) holds the lock file while this process's threads and timeouts pile up
//! against it.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use vaire::db::{self, DbLock, LockError};

/// The environment variable that turns this test binary into a lock-holding worker.
const WORKER_ENV: &str = "VAIRE_DB_LOCK_WORKER";

/// The wait limit is process-wide and both tests set it, so they take turns.
static POLICY: Mutex<()> = Mutex::new(());

/// The worker half: take the lock at `<lock>` and hold it until the parent drops the
/// `release` marker in `<signals>`. A no-op in a normal run.
#[test]
fn lock_worker() {
    let Ok(spec) = std::env::var(WORKER_ENV) else {
        return;
    };
    let (lock, signals) = spec
        .split_once('|')
        .expect("worker spec is <lock>|<signals>");
    let signals = Path::new(signals);
    db::set_lock_wait(db::lock_wait_from_env(None).expect("lock timeout"));
    let _held = DbLock::acquire(Path::new(lock), "the test lock").expect("worker takes the lock");
    std::fs::write(signals.join("ready"), b"").expect("ready marker");
    let deadline = Instant::now() + Duration::from_secs(120);
    while !signals.join("release").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Threads of one process asking for the same lock at once share one acquisition.
///
/// Each thread keeps its share until every thread has one, which only a shared acquisition
/// can satisfy. Two separate acquisitions — what a lookup-then-insert registry allowed, when
/// two threads both missed each other's entry — would leave the second waiting on the first's
/// OS lock until it timed out.
#[test]
fn threads_asking_at_once_share_one_acquisition() {
    const THREADS: usize = 8;
    let _turn = POLICY.lock().unwrap_or_else(PoisonError::into_inner);
    wait_for_no_waiters();
    db::set_lock_wait(Some(Duration::from_secs(10)));

    let dir = tempfile::tempdir().expect("tempdir");
    let lock = dir.path().join("test.lock");
    let mut holder = spawn_holder(&lock, dir.path(), None);
    wait_until_ready(&mut holder, dir.path());

    // Closed until every thread has reported, so no thread can let go of its share early.
    let gate = Arc::new(Mutex::new(()));
    let closed = gate.lock().expect("gate");
    let (report, reports) = mpsc::channel();
    let threads: Vec<_> = (0..THREADS)
        .map(|_| {
            let (lock, report, gate) = (lock.clone(), report.clone(), Arc::clone(&gate));
            std::thread::spawn(move || match DbLock::acquire(&lock, "the test lock") {
                Ok(share) => {
                    report.send(Ok(())).expect("report");
                    let _open = gate.lock().unwrap_or_else(PoisonError::into_inner);
                    drop(share);
                }
                Err(e) => report.send(Err(format!("{e:?}"))).expect("report"),
            })
        })
        .collect();

    // Give every thread the chance to queue behind the worker. One that has not simply takes
    // the lock uncontended after the release; the assertion below holds either way.
    std::thread::sleep(Duration::from_millis(300));
    release(dir.path());

    let outcomes: Vec<_> = (0..THREADS)
        .map(|_| {
            reports
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|_| Err("no report within 30s".to_string()))
        })
        .collect();
    drop(closed);
    for thread in threads {
        thread.join().expect("thread exits");
    }
    db::set_lock_wait(None);

    let failed: Vec<_> = outcomes.into_iter().filter_map(Result::err).collect();
    assert!(
        failed.is_empty(),
        "every thread holds a share at the same time: {failed:?}"
    );
    assert!(finish(holder).success());
}

/// Callers that give up on a lock another process holds leave one waiting thread between
/// them, not one each — and when the holder lets go, that thread releases the lock rather
/// than keeping it for nobody.
#[test]
fn callers_that_time_out_share_one_waiting_thread() {
    let _turn = POLICY.lock().unwrap_or_else(PoisonError::into_inner);
    wait_for_no_waiters();

    let dir = tempfile::tempdir().expect("tempdir");
    let lock = dir.path().join("test.lock");
    let mut holder = spawn_holder(&lock, dir.path(), None);
    wait_until_ready(&mut holder, dir.path());

    db::set_lock_wait(Some(Duration::from_millis(50)));
    for attempt in 0..20 {
        match DbLock::acquire(&lock, "the test lock") {
            Err(LockError::TimedOut(_)) => {}
            other => panic!(
                "attempt {attempt}: expected a timeout while the worker holds the lock, got {:?}",
                other.map(|_| ())
            ),
        }
    }
    db::set_lock_wait(None);
    assert_eq!(
        db::waiter_threads(),
        1,
        "twenty timed-out callers leave one waiting thread behind, not twenty"
    );

    // The holder lets go. The waiting thread gets the lock with nobody left to take it, and
    // must hand it straight back — proven by another process taking it, with a bound so a
    // kept lock fails the test rather than hanging it.
    release(dir.path());
    assert!(finish(holder).success());
    wait_for_no_waiters();

    let second = dir.path().join("second");
    std::fs::create_dir(&second).expect("second signal dir");
    let mut next = spawn_holder(&lock, &second, Some(("VAIRE_LOCK_TIMEOUT", "10")));
    wait_until_ready(&mut next, &second);
    release(&second);
    assert!(finish(next).success());
}

/// Start a worker holding the lock at `lock`, coordinating through `signals`.
fn spawn_holder(lock: &Path, signals: &Path, env: Option<(&str, &str)>) -> Child {
    let mut cmd = Command::new(std::env::current_exe().expect("test binary path"));
    cmd.args(["lock_worker", "--exact", "--quiet"])
        .env(
            WORKER_ENV,
            format!("{}|{}", lock.display(), signals.display()),
        )
        .env_remove("VAIRE_LOCK_TIMEOUT")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some((key, value)) = env {
        cmd.env(key, value);
    }
    cmd.spawn().expect("spawn worker")
}

/// Block until the worker reports that it holds the lock.
fn wait_until_ready(worker: &mut Child, signals: &Path) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !signals.join("ready").exists() {
        if let Some(status) = worker.try_wait().expect("try_wait") {
            panic!("the worker exited before it took the lock: {status}");
        }
        assert!(Instant::now() < deadline, "the worker never took the lock");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Tell the worker to let go.
fn release(signals: &Path) {
    std::fs::write(signals.join("release"), b"").expect("release marker");
}

/// Wait for a worker, failing the test rather than hanging it.
fn finish(mut worker: Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = worker.try_wait().expect("try_wait") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = worker.kill();
            panic!("the worker did not exit within 60s");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Block until no waiting thread from an earlier test is still alive, so counts start at zero.
fn wait_for_no_waiters() {
    let deadline = Instant::now() + Duration::from_secs(30);
    while db::waiter_threads() != 0 {
        assert!(
            Instant::now() < deadline,
            "a waiting thread outlived its lock"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
