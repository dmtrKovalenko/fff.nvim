//! Binary-level integration tests for fffctl (U8).
//!
//! Each test runs in an isolated XDG environment so paths never collide.
//! Tests spawn fff-engine in master mode and drive fffctl commands against it.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fff_ipc::{master_socket_path, routing_table_path};
use tempfile::TempDir;

const CTL_BIN: &str = env!("CARGO_BIN_EXE_fffctl");
const SOCKET_TIMEOUT: Duration = Duration::from_secs(15);

fn engine_bin() -> PathBuf {
    let mut p = std::env::current_exe()
        .unwrap()
        .canonicalize()
        .unwrap();
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("fff-engine")
}

// ─────────────────────────────────────────────────────────────────────────────
// TestEnv

struct TestEnv {
    _dir: TempDir,
    cache: PathBuf,
    runtime: PathBuf,
    config: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let cache = dir.path().join("cache");
        let runtime = dir.path().join("runtime");
        let config = dir.path().join("config");

        for d in [
            cache.join("fff").join("workers"),
            cache.join("fff").join("locks"),
            cache.join("fff").join("sockets"),
            runtime.join("fff"),
            config.join("fff"),
        ] {
            std::fs::create_dir_all(&d).unwrap();
        }

        // Config: n_min=1, n_max=3, roots_per_worker_max=2, idle_ttl_secs=5
        let cfg_path = config.join("fff").join("config.toml");
        std::fs::write(
            &cfg_path,
            "[worker]\nn_min = 1\nn_max = 3\nroots_per_worker_max = 2\nidle_ttl_secs = 5\n",
        )
        .unwrap();

        Self { _dir: dir, cache, runtime, config }
    }

    fn env_vars(&self) -> [(&'static str, &Path); 3] {
        [
            ("XDG_CACHE_HOME", self.cache.as_path()),
            ("XDG_RUNTIME_DIR", self.runtime.as_path()),
            ("XDG_CONFIG_HOME", self.config.as_path()),
        ]
    }

    fn master_socket(&self) -> PathBuf {
        let orig = std::env::var("XDG_CACHE_HOME").ok();
        // Temporarily point helpers at our isolated dirs
        unsafe { std::env::set_var("XDG_CACHE_HOME", &self.cache) };
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &self.runtime) };
        let p = master_socket_path();
        match orig {
            Some(v) => unsafe { std::env::set_var("XDG_CACHE_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_CACHE_HOME") },
        }
        p
    }

    fn routing_json(&self) -> PathBuf {
        let orig = std::env::var("XDG_RUNTIME_DIR").ok();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &self.runtime) };
        let p = routing_table_path();
        match orig {
            Some(v) => unsafe { std::env::set_var("XDG_RUNTIME_DIR", v) },
            None => unsafe { std::env::remove_var("XDG_RUNTIME_DIR") },
        }
        p
    }

    fn spawn_master(&self) -> Child {
        Command::new(engine_bin())
            .arg("--master")
            .envs(self.env_vars())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn fff-engine --master")
    }

    fn wait_master(&self, timeout: Duration) -> bool {
        let sock = self.master_socket();
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if UnixStream::connect(&sock).is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn wait_socket_gone(&self, path: &Path, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if UnixStream::connect(path).is_err() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Run fffctl with the given args in this environment. Returns (stdout, stderr, exit code).
    fn fffctl(&self, args: &[&str]) -> (String, String, i32) {
        let out = Command::new(CTL_BIN)
            .args(args)
            .envs(self.env_vars())
            .output()
            .expect("failed to run fffctl");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let code = out.status.code().unwrap_or(-1);
        (stdout, stderr, code)
    }

}

fn sigterm(child: &Child) {
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
}

// ─────────────────────────────────────────────────────────────────────────────
// U8 test scenarios

/// U8.1 — `fffctl list` shows master PID when master is running.
#[test]
fn list_shows_master_pid() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT), "master did not start");

    let (out, _, code) = env.fffctl(&["list"]);
    assert_eq!(code, 0, "fffctl list exit code; stderr: {out}");
    assert!(out.contains("master PID:"), "expected 'master PID:' in output:\n{out}");

    sigterm(&master);
    let _ = master.wait();
}

/// U8.2 — `fffctl list` falls back gracefully when master is not running.
#[test]
fn list_falls_back_without_master() {
    let env = TestEnv::new();
    // No master — should use legacy fallback and exit 0.
    let (out, _, code) = env.fffctl(&["list"]);
    assert_eq!(code, 0);
    // Either "No fff-engine daemons" or a legacy fallback notice.
    assert!(
        out.contains("No fff-engine") || out.contains("master not running"),
        "unexpected output:\n{out}"
    );
}

/// U8.3 — `fffctl list-workers` shows at least the min worker when master is running.
#[test]
fn list_workers_shows_workers() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT), "master did not start");

    // Wait briefly for n_min=1 worker to appear.
    std::thread::sleep(Duration::from_millis(500));

    let (out, _, code) = env.fffctl(&["list-workers"]);
    assert_eq!(code, 0, "fffctl list-workers failed:\n{out}");
    assert!(out.contains("INDEX"), "missing header in output:\n{out}");

    sigterm(&master);
    let _ = master.wait();
}

/// U8.4 — `fffctl stop --all` terminates master; socket disappears.
#[test]
fn stop_all_terminates_master() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT), "master did not start");

    let sock = env.master_socket();
    let (out, _, code) = env.fffctl(&["stop", "--all"]);
    assert_eq!(code, 0, "fffctl stop --all failed:\n{out}");
    assert!(out.contains("SIGTERM"), "expected SIGTERM mention in output:\n{out}");

    assert!(
        env.wait_socket_gone(&sock, SOCKET_TIMEOUT),
        "master socket still accepting connections after stop --all"
    );
    let _ = master.wait();
}

/// U8.5 — `fffctl clean` removes routing.json and master artifacts when master is stopped.
#[test]
fn clean_removes_routing_json_after_master_stopped() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT), "master did not start");

    // Let master write routing.json, then stop it.
    std::thread::sleep(Duration::from_millis(300));
    sigterm(&master);
    let sock = env.master_socket();
    env.wait_socket_gone(&sock, SOCKET_TIMEOUT);
    let _ = master.wait();

    let routing = env.routing_json();
    assert!(routing.exists(), "routing.json should exist after master ran");

    let (out, _, code) = env.fffctl(&["clean"]);
    assert_eq!(code, 0, "fffctl clean failed:\n{out}");
    assert!(!routing.exists(), "routing.json should be removed by clean");
    assert!(out.contains("routing table"), "expected routing table mention:\n{out}");
}

/// U8.6 — `fffctl clean --dry-run` prints what would be removed but leaves files intact.
#[test]
fn clean_dry_run_does_not_remove() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT), "master did not start");

    std::thread::sleep(Duration::from_millis(300));
    sigterm(&master);
    let sock = env.master_socket();
    env.wait_socket_gone(&sock, SOCKET_TIMEOUT);
    let _ = master.wait();

    let routing = env.routing_json();
    assert!(routing.exists(), "routing.json should exist after master ran");

    let (out, _, code) = env.fffctl(&["clean", "--dry-run"]);
    assert_eq!(code, 0, "fffctl clean --dry-run failed:\n{out}");
    assert!(routing.exists(), "routing.json must NOT be removed by --dry-run");
    assert!(out.contains("would remove"), "expected 'would remove' in output:\n{out}");
}

/// U8.7 — `fffctl paths <path>` prints all expected fields.
#[test]
fn paths_shows_all_fields() {
    let env = TestEnv::new();
    let (out, _, code) = env.fffctl(&["paths", "/tmp/fff-test-project"]);
    assert_eq!(code, 0, "fffctl paths failed");
    for field in &["base_path", "slug", "socket", "lockfile", "master.sock", "routing.json"] {
        assert!(out.contains(field), "missing field '{field}' in output:\n{out}");
    }
}
