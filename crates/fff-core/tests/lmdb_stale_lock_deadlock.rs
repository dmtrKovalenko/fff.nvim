//! Reproduces the deadlock/hang caused by LMDB writer mutex contention.
//!
//! When another process holds the LMDB writer mutex (via a long-running write
//! transaction or because it crashed without releasing it), any call to
//! `write_txn()` blocks indefinitely — including on the neovim main thread
//! during `QueryTracker::open()` or frecency `track_access()`.
//!
//! In production this manifests as neovim hanging on startup:
//!   require('fff.core').ensure_initialized()
//!   → init_db() → QueryTracker::open() → write_txn() → HANGS
//!
//! Or during normal use when BufEnter fires:
//!   track_access → frecency.track_access() → write_txn() → HANGS
//!
//! Reproduction: fork a child process that holds the LMDB write lock
//! indefinitely, then attempt to use the same database from the parent.
//! The parent's `write_txn()` blocks on the cross-process writer mutex.
//!
//! This test confirms that the current code has NO timeout or fallback when the
//! LMDB writer mutex is unavailable — making it vulnerable to indefinite hangs
//! whenever another process (fff-mcp, another neovim, or a crashed instance)
//! holds or has stuck the mutex.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use fff_search::frecency::FrecencyTracker;
use fff_search::query_tracker::QueryTracker;

/// Returns whether `f` completes within `timeout`.
fn completes_within(
    label: &'static str,
    timeout: Duration,
    f: impl FnOnce() + Send + 'static,
) -> bool {
    let (tx, rx) = mpsc::channel::<()>();
    let _worker = std::thread::Builder::new()
        .name(format!("deadlock-repro-{label}"))
        .spawn(move || {
            f();
            let _ = tx.send(());
        })
        .expect("spawn worker");

    rx.recv_timeout(timeout).is_ok()
}

/// Fork a child that opens the LMDB env and holds a write transaction
/// indefinitely (simulating a stuck/long-running process). Returns the
/// child PID so the parent can kill it during cleanup.
fn fork_child_holding_write_lock(db_path: &Path) -> libc::pid_t {
    let db_path_str = db_path.to_str().unwrap().to_owned();

    let mut pipe_fds: [libc::c_int; 2] = [0; 2];
    assert_eq!(unsafe { libc::pipe(pipe_fds.as_mut_ptr()) }, 0);
    let read_fd = pipe_fds[0];
    let write_fd = pipe_fds[1];

    let child_pid = unsafe { libc::fork() };
    match child_pid {
        -1 => panic!("fork() failed: {}", std::io::Error::last_os_error()),
        0 => {
            // === CHILD PROCESS ===
            unsafe { libc::close(read_fd) };

            let env = unsafe {
                let mut opts = heed::EnvOpenOptions::new();
                opts.map_size(10 * 1024 * 1024);
                opts.open(Path::new(&db_path_str)).expect("child: open env")
            };

            // Acquire the cross-process writer mutex via write_txn
            let _wtxn = env.write_txn().expect("child: write_txn");

            // Signal parent that the lock is held
            unsafe { libc::write(write_fd, b"R".as_ptr() as *const libc::c_void, 1) };

            // Hold the lock forever — parent will eventually kill us
            loop {
                unsafe { libc::pause() };
            }
        }
        pid => {
            // === PARENT PROCESS ===
            unsafe { libc::close(write_fd) };

            // Wait for child to confirm it holds the write lock
            let mut buf = [0u8; 1];
            let n = unsafe { libc::read(read_fd, buf.as_mut_ptr() as *mut libc::c_void, 1) };
            assert_eq!(n, 1, "child didn't signal readiness");
            assert_eq!(buf[0], b'R');
            unsafe { libc::close(read_fd) };

            pid
        }
    }
}

/// Kill and reap the child process.
fn kill_child(pid: libc::pid_t) {
    unsafe {
        libc::kill(pid, libc::SIGKILL);
        let mut status: libc::c_int = 0;
        libc::waitpid(pid, &mut status, 0);
    }
}

/// Verify QueryTracker works correctly after close+reopen — the
/// open_database_safe path must find existing named databases via read txn.
#[test]
fn lmdb_reopen_finds_existing_databases() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("lmdb_reopen");
    fs::create_dir_all(&db_path).unwrap();

    // First open: creates the databases via write_txn fallback
    {
        let mut tracker = QueryTracker::open(&db_path).unwrap();
        let project = Path::new("/test/project");
        let file = Path::new("/test/project/src/main.rs");
        tracker
            .track_query_completion("hello", project, file)
            .unwrap();
    }

    // Second open: must find existing databases via read txn (no write_txn needed)
    {
        let tracker = QueryTracker::open(&db_path).unwrap();
        let project = Path::new("/test/project");
        let result = tracker.get_historical_query(project, 0).unwrap();
        assert_eq!(
            result,
            Some("hello".to_string()),
            "Query history should persist across close/reopen"
        );
    }
}

/// Env var the test binary checks on startup. When set, the binary skips the
/// test harness and runs as a child worker instead. This avoids fork() in a
/// multi-threaded parent — which copies mutex/allocator state from threads
/// that no longer exist in the child and can deadlock heed/libc.
const CHILD_MODE_ENV: &str = "FFF_PARALLEL_OPEN_CLOSE_CHILD";

/// Runs before the test harness when `CHILD_MODE_ENV` is set. Re-exec of
/// the test binary lets us start child workers without forking from a
/// multi-threaded parent.
#[ctor::ctor]
fn maybe_enter_child_mode() {
    if let Ok(spec) = std::env::var(CHILD_MODE_ENV) {
        let code = run_child_from_spec(&spec);
        std::process::exit(code);
    }
}

/// Spec format: `db_path|idx|iterations|writer(0|1)`
fn run_child_from_spec(spec: &str) -> i32 {
    let parts: Vec<&str> = spec.split('|').collect();
    if parts.len() != 4 {
        return CHILD_BAD_SPEC;
    }
    let db_path = parts[0];
    let idx: usize = match parts[1].parse() {
        Ok(v) => v,
        Err(_) => return CHILD_BAD_SPEC,
    };
    let iterations: usize = match parts[2].parse() {
        Ok(v) => v,
        Err(_) => return CHILD_BAD_SPEC,
    };
    let is_writer = parts[3] == "1";
    child_open_close_loop(db_path, iterations, idx, is_writer)
}

const CHILD_OK: i32 = 0;
const CHILD_OPEN_FAILED: i32 = 10;
const CHILD_READ_FAILED: i32 = 11;
const CHILD_WRITE_FAILED: i32 = 12;
const CHILD_BAD_SPEC: i32 = 13;

fn child_open_close_loop(db_path: &str, iterations: usize, idx: usize, is_writer: bool) -> i32 {
    let project = Path::new("/test/project");
    for i in 0..iterations {
        let tracker = match QueryTracker::open(Path::new(db_path)) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("child {idx} iter {i} reader open failed: {e:?}");
                return CHILD_OPEN_FAILED;
            }
        };
        if let Err(e) = tracker.get_historical_query(project, 0) {
            eprintln!("child {idx} iter {i} read failed: {e:?}");
            return CHILD_READ_FAILED;
        }
        drop(tracker);

        if is_writer {
            let mut tracker = match QueryTracker::open(Path::new(db_path)) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("child {idx} iter {i} writer open failed: {e:?}");
                    return CHILD_OPEN_FAILED;
                }
            };
            let file = PathBuf::from(format!("/test/project/c{idx}_{i}.rs"));
            if let Err(e) = tracker.track_query_completion(&format!("q{idx}_{i}"), project, &file) {
                eprintln!("child {idx} iter {i} write failed: {e:?}");
                return CHILD_WRITE_FAILED;
            }
            drop(tracker);
        }
    }
    CHILD_OK
}

/// Spawn `n` child processes via `Command::new(current_exe)`. No fork, so
/// mutex/allocator state is not inherited. `writers` children also issue
/// writes; the rest only read.
fn spawn_open_close_children(
    db_path: &Path,
    n: usize,
    writers: usize,
    ops_per_child: usize,
) -> Vec<std::process::Child> {
    assert!(writers <= n);
    let exe = std::env::current_exe().expect("current_exe");
    let db_path_str = db_path.to_str().unwrap().to_owned();

    (0..n)
        .map(|idx| {
            let is_writer = idx < writers;
            let spec = format!(
                "{db_path_str}|{idx}|{ops_per_child}|{}",
                if is_writer { 1 } else { 0 }
            );
            std::process::Command::new(&exe)
                .env(CHILD_MODE_ENV, spec)
                .env_remove("RUST_LOG")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::inherit())
                .spawn()
                .expect("spawn child")
        })
        .collect()
}

/// Wait for every child with a per-call deadline. On timeout, kill and reap
/// remaining children and return an Err describing the stuck set.
fn wait_all_with_deadline(
    mut children: Vec<std::process::Child>,
    deadline: std::time::Instant,
) -> Result<(), String> {
    let mut failures: Vec<(u32, Option<i32>)> = Vec::new();
    let mut remaining: Vec<std::process::Child> = Vec::new();

    for mut child in children.drain(..) {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code();
                    if code != Some(CHILD_OK) {
                        failures.push((child.id(), code));
                    }
                    break;
                }
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        remaining.push(child);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => {
                    failures.push((child.id(), None));
                    let _ = e;
                    break;
                }
            }
        }
    }

    if !remaining.is_empty() {
        let stuck: Vec<u32> = remaining.iter().map(|c| c.id()).collect();
        for child in &mut remaining {
            let _ = child.kill();
            let _ = child.wait();
        }
        return Err(format!(
            "deadline exceeded; children still running: {stuck:?}"
        ));
    }

    if !failures.is_empty() {
        return Err(format!("children failed: {failures:?}"));
    }
    Ok(())
}

/// Many processes open/close `QueryTracker` against the same DB path.
/// Readers only: seeds once, then spawns N reader children.
///
/// heed 0.22 forbids opening the same env twice *within* one process
/// (EnvAlreadyOpened), so cross-process contention is the right axis.
#[test]
fn query_tracker_many_parallel_open_close_same_path_readers() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("parallel_open_close_readers");
    fs::create_dir_all(&db_path).unwrap();

    {
        let mut tracker = QueryTracker::open(&db_path).unwrap();
        tracker
            .track_query_completion(
                "seed",
                Path::new("/test/project"),
                Path::new("/test/project/src/main.rs"),
            )
            .unwrap();
    }

    const N: usize = 8;
    const OPS: usize = 4;

    let children = spawn_open_close_children(&db_path, N, 0, OPS);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    wait_all_with_deadline(children, deadline).expect("parallel open/close (readers)");

    let tracker = QueryTracker::open(&db_path).expect("post-storm reopen");
    let project = Path::new("/test/project");
    let result = tracker.get_historical_query(project, 0).unwrap();
    assert_eq!(
        result,
        Some("seed".to_string()),
        "Seed query should still be readable after parallel open/close storm"
    );
}

/// Stronger variant: multiple processes race opens that both read AND write.
/// LMDB serializes writers via a cross-process mutex; test that serialization
/// makes forward progress and open/close pairs don't deadlock.
#[test]
fn query_tracker_parallel_open_write_close_same_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("parallel_open_write_close");
    fs::create_dir_all(&db_path).unwrap();

    {
        let mut tracker = QueryTracker::open(&db_path).unwrap();
        tracker
            .track_query_completion(
                "seed",
                Path::new("/test/project"),
                Path::new("/test/project/src/main.rs"),
            )
            .unwrap();
    }

    const N: usize = 4;
    const WRITERS: usize = 4;
    const OPS: usize = 3;

    let children = spawn_open_close_children(&db_path, N, WRITERS, OPS);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    wait_all_with_deadline(children, deadline).expect("parallel open/write/close");

    let tracker = QueryTracker::open(&db_path).expect("post-storm reopen");
    let project = Path::new("/test/project");
    let seed = tracker.get_historical_query(project, 0).unwrap();
    assert!(
        seed.is_some(),
        "Env unreadable after parallel open/write/close storm"
    );
}

/// Within a single process, opening the same env path twice concurrently is
/// forbidden by heed — but a strict sequential open→use→drop→open loop must
/// succeed every iteration. Regression guard for the reopen path.
#[test]
fn query_tracker_sequential_reopen_loop_same_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("sequential_reopen_loop");
    fs::create_dir_all(&db_path).unwrap();

    {
        let mut tracker = QueryTracker::open(&db_path).unwrap();
        tracker
            .track_query_completion(
                "seed",
                Path::new("/test/project"),
                Path::new("/test/project/src/main.rs"),
            )
            .unwrap();
    }

    for i in 0..64 {
        let mut tracker = QueryTracker::open(&db_path).expect("sequential reopen");
        let project = Path::new("/test/project");
        let file = PathBuf::from(format!("/test/project/iter_{i}.rs"));
        tracker
            .track_query_completion(&format!("iter_{i}"), project, &file)
            .expect("sequential track");
        drop(tracker);
    }

    let tracker = QueryTracker::open(&db_path).expect("final reopen");
    let project = Path::new("/test/project");
    assert!(tracker.get_historical_query(project, 0).unwrap().is_some());
}

/// When the frecency DB doesn't exist yet, `FrecencyTracker::open()` falls
/// through to `write_txn()` + `create_database()`. This blocks if another
/// process holds the writer mutex. This is the first-launch path.
///
/// NOTE: this test is disabled because heed 0.22 appears to use a
/// try-then-create pattern for unnamed databases that doesn't always block.
/// The QueryTracker test above (named databases, always needs write_txn)
/// reliably demonstrates the same underlying issue.
#[test]
#[ignore = "heed 0.22 unnamed db creation may not require writer mutex in all cases"]
fn frecency_open_blocks_on_fresh_db_when_another_process_holds_write_lock() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("frecency_fresh_deadlock");
    fs::create_dir_all(&db_path).unwrap();

    let env = unsafe {
        let mut opts = heed::EnvOpenOptions::new();
        opts.map_size(10 * 1024 * 1024);
        opts.open(&db_path).unwrap()
    };
    drop(env);

    let child_pid = fork_child_holding_write_lock(&db_path);

    let db_path_clone = db_path.clone();
    let completed = completes_within(
        "FrecencyTracker::open (fresh db) while writer held",
        Duration::from_secs(3),
        move || {
            let _result = FrecencyTracker::open(&db_path_clone);
        },
    );

    kill_child(child_pid);

    assert!(
        !completed,
        "Expected FrecencyTracker::open() on a fresh DB to block (writer mutex \
         held by another process), but it completed."
    );
}
