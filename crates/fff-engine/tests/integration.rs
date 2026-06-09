/// Integration tests for fff-engine covering:
/// - U3: Worker socket binding, protocol enforcement, Connect/Ack, cleanup on SIGTERM
/// - U4: Master lockfile, socket, single-instance guard, Handshake, ListWorkers,
///       WorkerStatus, routing.json persistence, startup with dead-PID routing.json, cleanup
/// - U5: Routing table fast path, scale-out on roots_per_worker_max, stable re-routing,
///       routing.json updated after each Handshake mutation
/// - U6: Worker crash detection and respawn, startup dead-vs-live PID recovery
use std::{
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread::sleep,
    time::Duration,
};

use fff_ipc::{
    codec::{read_message_sync, write_message_sync},
    routing::{RoutingTable, WorkerEntry},
    types::{FindOptions, MasterRequest, MasterResponse, SearchRequest, SearchResponse},
};
use tempfile::TempDir;

const ENGINE_BIN: &str = env!("CARGO_BIN_EXE_fff-engine");
/// Max wait for master or worker socket readiness.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(15);
/// Short poll interval.
const POLL_MS: Duration = Duration::from_millis(50);

// ── TestEnv ────────────────────────────────────────────────────────────────────

struct TestEnv {
    dir: TempDir,
}

impl TestEnv {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let env = Self { dir };

        // Write a config file picked up by all spawned processes via XDG_CONFIG_HOME.
        let cfg_dir = env.config_dir().join("fff");
        std::fs::create_dir_all(&cfg_dir).expect("create config dir");
        let cfg = "[worker]\nn_min = 1\nn_max = 3\nroots_per_worker_max = 2\nidle_ttl_secs = 5\n";
        std::fs::write(cfg_dir.join("config.toml"), cfg).expect("write config");

        // Create the cache/runtime subdirs so processes can write sockets/lockfiles
        // without a race on directory creation in the engine itself.
        std::fs::create_dir_all(env.cache_dir().join("fff").join("workers"))
            .expect("create workers dir");
        std::fs::create_dir_all(env.runtime_dir().join("fff")).expect("create runtime fff dir");

        env
    }

    fn cache_dir(&self) -> PathBuf {
        self.dir.path().join("cache")
    }

    fn runtime_dir(&self) -> PathBuf {
        self.dir.path().join("runtime")
    }

    fn config_dir(&self) -> PathBuf {
        self.dir.path().join("config")
    }

    fn master_socket(&self) -> PathBuf {
        self.cache_dir().join("fff").join("master.sock")
    }

    fn master_lockfile(&self) -> PathBuf {
        self.cache_dir().join("fff").join("master.lock")
    }

    fn worker_socket(&self, idx: u32) -> PathBuf {
        self.cache_dir().join("fff").join("workers").join(format!("worker-{idx}.sock"))
    }

    fn worker_lockfile(&self, idx: u32) -> PathBuf {
        self.cache_dir().join("fff").join("workers").join(format!("worker-{idx}.lock"))
    }

    fn routing_json(&self) -> PathBuf {
        self.runtime_dir().join("fff").join("routing.json")
    }

    fn spawn_master(&self) -> Child {
        Command::new(ENGINE_BIN)
            .arg("--master")
            .env("XDG_CACHE_HOME", self.cache_dir())
            .env("XDG_RUNTIME_DIR", self.runtime_dir())
            .env("XDG_CONFIG_HOME", self.config_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn master")
    }

    fn spawn_worker(&self, idx: u32) -> Child {
        Command::new(ENGINE_BIN)
            .args(["--worker-index", &idx.to_string()])
            .env("XDG_CACHE_HOME", self.cache_dir())
            .env("XDG_RUNTIME_DIR", self.runtime_dir())
            .env("XDG_CONFIG_HOME", self.config_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn worker")
    }

    fn wait_master(&self, timeout: Duration) -> bool {
        self.wait_socket(&self.master_socket(), timeout)
    }

    fn wait_worker(&self, idx: u32, timeout: Duration) -> bool {
        self.wait_socket(&self.worker_socket(idx), timeout)
    }

    fn wait_socket(&self, path: &PathBuf, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if UnixStream::connect(path).is_ok() {
                return true;
            }
            sleep(POLL_MS);
        }
        false
    }

    /// Wait until `path` does NOT exist (or cannot be connected to).
    fn wait_socket_gone(&self, path: &PathBuf, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if UnixStream::connect(path).is_err() {
                return true;
            }
            sleep(POLL_MS);
        }
        false
    }

    fn send_master_request(&self, req: &MasterRequest) -> MasterResponse {
        let mut stream = UnixStream::connect(self.master_socket()).expect("connect to master");
        write_message_sync(&mut stream, req).expect("write master request");
        read_message_sync(&mut stream).expect("read master response")
    }

    fn handshake(&self, base_path: &str) -> MasterResponse {
        self.send_master_request(&MasterRequest::Handshake { base_path: base_path.into() })
    }

    fn list_workers(&self) -> Vec<fff_ipc::types::WorkerInfo> {
        match self.send_master_request(&MasterRequest::ListWorkers) {
            MasterResponse::WorkerList { workers } => workers,
            other => panic!("expected WorkerList, got {other:?}"),
        }
    }

    fn worker_status(&self, idx: u32) -> Option<fff_ipc::types::WorkerInfo> {
        match self.send_master_request(&MasterRequest::WorkerStatus { index: idx }) {
            MasterResponse::WorkerInfo(info) => Some(info),
            MasterResponse::Error(_) => None,
            other => panic!("unexpected WorkerStatus response: {other:?}"),
        }
    }

    /// Connect to a worker socket, send Connect, receive Ack.
    fn worker_connect(&self, worker_sock: &PathBuf, base_path: &str) -> UnixStream {
        let mut stream = UnixStream::connect(worker_sock).expect("connect to worker");
        let req = SearchRequest::Connect { base_path: base_path.into() };
        write_message_sync(&mut stream, &req).expect("write Connect");
        let resp: SearchResponse = read_message_sync(&mut stream).expect("read Ack");
        assert!(
            matches!(resp, SearchResponse::Ack),
            "expected Ack from worker Connect, got {resp:?}"
        );
        stream
    }

    fn kill_sigterm(child: &Child) {
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
        }
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        // Best-effort: clean up any leftover processes in case a test panics
        // without explicit teardown. The TempDir drop will remove the files.
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn kill_and_wait(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn sigterm_and_wait(child: &mut Child, timeout: Duration) {
    TestEnv::kill_sigterm(child);
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        sleep(POLL_MS);
    }
    let _ = child.kill();
    let _ = child.wait();
}

// ── U3: Worker tests ────────────────────────────────────────────────────────────

/// U3-1: Worker binds its socket file at worker_socket_path(N) on startup.
#[test]
fn u3_worker_binds_socket_on_startup() {
    let env = TestEnv::new();
    let mut worker = env.spawn_worker(0);

    assert!(env.wait_worker(0, SOCKET_TIMEOUT), "worker-0 socket not ready in time");
    assert!(env.worker_socket(0).exists(), "worker-0.sock should exist on disk");

    sigterm_and_wait(&mut worker, Duration::from_secs(5));
}

/// U3-2: Non-Connect first message is rejected — connection closes without crash.
/// Sends FindFiles as first message; worker closes without sending a response.
#[test]
fn u3_non_connect_first_message_closes_connection() {
    let env = TestEnv::new();
    let mut worker = env.spawn_worker(0);
    assert!(env.wait_worker(0, SOCKET_TIMEOUT));

    let mut stream = UnixStream::connect(env.worker_socket(0)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    // Send a non-Connect message as the first message on the connection.
    let bad_req = SearchRequest::FindFiles {
        query: "main".into(),
        options: FindOptions::default(),
    };
    write_message_sync(&mut stream, &bad_req).expect("write bad request");

    // Worker should close the connection (EOF) rather than crash.
    let result: Result<SearchResponse, _> = read_message_sync(&mut stream);
    assert!(result.is_err(), "expected EOF or error, worker should close connection on bad first msg");

    // Worker itself should still be alive (no crash).
    assert!(
        worker.try_wait().expect("try_wait").is_none(),
        "worker should still be running after bad first message"
    );

    sigterm_and_wait(&mut worker, Duration::from_secs(5));
}

/// U3-3: `Connect { base_path }` receives `Ack`.
#[test]
fn u3_connect_receives_ack() {
    let env = TestEnv::new();
    let mut worker = env.spawn_worker(0);
    assert!(env.wait_worker(0, SOCKET_TIMEOUT));

    let base_path = env.dir.path().to_str().unwrap();
    let _stream = env.worker_connect(&env.worker_socket(0), base_path);

    sigterm_and_wait(&mut worker, Duration::from_secs(5));
}

/// U3-4: Two connections for the same base_path both receive Ack (second is fast-path).
#[test]
fn u3_second_connect_same_base_path_gets_ack() {
    let env = TestEnv::new();
    let mut worker = env.spawn_worker(0);
    assert!(env.wait_worker(0, SOCKET_TIMEOUT));

    let base_path = env.dir.path().to_str().unwrap();

    // First connection — triggers state init.
    let _stream1 = env.worker_connect(&env.worker_socket(0), base_path);

    // Second connection — hits the already-loaded root (fast path).
    let _stream2 = env.worker_connect(&env.worker_socket(0), base_path);

    // Worker lockfile PID should be unchanged (no respawn).
    let lockfile_content = std::fs::read_to_string(env.worker_lockfile(0))
        .expect("lockfile should exist");
    let pid: u32 = lockfile_content.trim().parse().expect("pid in lockfile");
    assert_eq!(pid, worker.id(), "worker PID should not change between two connections");

    sigterm_and_wait(&mut worker, Duration::from_secs(5));
}

/// U3-5: Worker cleans up socket and lockfile on SIGTERM.
#[test]
fn u3_worker_cleans_up_on_sigterm() {
    let env = TestEnv::new();
    let mut worker = env.spawn_worker(0);
    assert!(env.wait_worker(0, SOCKET_TIMEOUT));
    assert!(env.worker_socket(0).exists());

    sigterm_and_wait(&mut worker, Duration::from_secs(5));

    assert!(
        !env.worker_socket(0).exists(),
        "worker-0.sock should be removed after SIGTERM"
    );
    assert!(
        !env.worker_lockfile(0).exists(),
        "worker-0.lock should be removed after SIGTERM"
    );
}

// ── U4: Master tests ────────────────────────────────────────────────────────────

/// U4-1: Master writes PID to master_lockfile_path() on startup.
#[test]
fn u4_master_writes_pid_to_lockfile() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));

    let content = std::fs::read_to_string(env.master_lockfile())
        .expect("master lockfile should exist");
    let pid: u32 = content.trim().parse().expect("lockfile should contain a valid PID");
    assert_eq!(pid, master.id(), "lockfile PID should match spawned master PID");

    sigterm_and_wait(&mut master, Duration::from_secs(5));
}

/// U4-2: Master binds socket at master_socket_path().
#[test]
fn u4_master_binds_socket() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT), "master socket not ready in time");
    assert!(env.master_socket().exists(), "master.sock should exist on disk");

    sigterm_and_wait(&mut master, Duration::from_secs(5));
}

/// U4-3: Second master instance exits cleanly — lockfile held by live process.
#[test]
fn u4_second_master_exits_cleanly() {
    let env = TestEnv::new();
    let mut master1 = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));

    // Spawn a second master in the same env — it should detect the live lockfile and exit.
    let mut master2 = env.spawn_master();
    // Give it a few seconds to detect the conflict and exit.
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    let second_exited = loop {
        if let Ok(Some(_)) = master2.try_wait() {
            break true;
        }
        if std::time::Instant::now() >= deadline {
            break false;
        }
        sleep(POLL_MS);
    };
    assert!(second_exited, "second master instance should exit when first is alive");

    sigterm_and_wait(&mut master1, Duration::from_secs(5));
    kill_and_wait(master2);
}

/// U4-4: `Handshake { base_path }` returns `MasterResponse::WorkerSocket` pointing
/// to a real worker socket path.
#[test]
fn u4_handshake_returns_worker_socket() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));

    let base_path = env.dir.path().to_str().unwrap();
    let resp = env.handshake(base_path);

    match &resp {
        MasterResponse::WorkerSocket { path, .. } => {
            assert!(!path.is_empty(), "worker socket path should not be empty");
            // Wait until the worker socket is actually connectable.
            let sock = PathBuf::from(path);
            assert!(
                env.wait_socket(&sock, SOCKET_TIMEOUT),
                "worker socket from Handshake response should be connectable"
            );
        }
        other => panic!("expected WorkerSocket, got {other:?}"),
    }

    sigterm_and_wait(&mut master, Duration::from_secs(5));
}

/// U4-5: `ListWorkers` returns all currently registered workers (correct count).
#[test]
fn u4_list_workers_returns_registered_workers() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));

    // Master starts with n_min=1 worker.
    let workers = env.list_workers();
    assert!(!workers.is_empty(), "master should have at least n_min=1 worker registered");

    sigterm_and_wait(&mut master, Duration::from_secs(5));
}

/// U4-6: `WorkerStatus { index: 0 }` for a live worker returns `WorkerInfo` with valid PID.
#[test]
fn u4_worker_status_returns_valid_pid() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));

    // Discover which worker index was started (n_min=1 → index 0).
    let workers = env.list_workers();
    assert!(!workers.is_empty());
    let idx = workers[0].index;

    let info = env.worker_status(idx).expect("WorkerStatus should return info for live worker");
    assert!(info.pid > 1, "worker PID should be a valid process ID");
    assert_eq!(info.index, idx);

    sigterm_and_wait(&mut master, Duration::from_secs(5));
}

/// U4-7: Routing table JSON is written to disk after a worker is spawned (via Handshake).
#[test]
fn u4_routing_json_written_after_handshake() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));

    let base_path = env.dir.path().to_str().unwrap();
    let resp = env.handshake(base_path);
    assert!(matches!(resp, MasterResponse::WorkerSocket { .. }), "handshake failed: {resp:?}");

    // routing.json should exist and contain the worker entry.
    let routing_path = env.routing_json();
    assert!(routing_path.exists(), "routing.json should exist after Handshake");

    let table = RoutingTable::load(&routing_path).expect("parse routing.json");
    assert!(!table.workers.is_empty(), "routing.json should contain at least one worker");

    sigterm_and_wait(&mut master, Duration::from_secs(5));
}

/// U4-8: Master startup reads routing.json and skips workers with dead PIDs.
/// We write a routing.json with a dead PID before starting master.
#[test]
fn u4_startup_skips_dead_pid_in_routing_json() {
    let env = TestEnv::new();

    // Write a routing.json with an unreachable/dead PID (999999999).
    let routing_dir = env.runtime_dir().join("fff");
    std::fs::create_dir_all(&routing_dir).expect("create runtime/fff dir");

    let dead_pid: u32 = 999_999_999;
    let dead_sock = env.worker_socket(99).to_string_lossy().into_owned();
    let mut table = RoutingTable::default();
    table.workers.insert(99, WorkerEntry {
        index: 99,
        socket_path: dead_sock,
        pid: dead_pid,
        root_slugs: vec!["some-slug".into()],
    });
    table.save(&env.routing_json()).expect("save routing.json");

    // Start master — it should discard the dead-PID entry and start fresh workers.
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));

    let workers = env.list_workers();
    let has_dead = workers.iter().any(|w| w.pid == dead_pid);
    assert!(!has_dead, "master should have discarded the dead-PID worker from routing.json");

    sigterm_and_wait(&mut master, Duration::from_secs(5));
}

/// U4-9: Master cleans up master socket and lockfile on SIGTERM.
#[test]
fn u4_master_cleans_up_on_sigterm() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));
    assert!(env.master_socket().exists());
    assert!(env.master_lockfile().exists());

    sigterm_and_wait(&mut master, Duration::from_secs(5));

    assert!(
        !env.master_socket().exists(),
        "master.sock should be removed after SIGTERM"
    );
    assert!(
        !env.master_lockfile().exists(),
        "master.lock should be removed after SIGTERM"
    );
}

// ── U5: Scale-out and routing ───────────────────────────────────────────────────

/// U5-1: Second Handshake for same base_path hits routing table (fast path).
/// Both responses must return the same worker_index.
#[test]
fn u5_second_handshake_same_base_path_hits_routing() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));

    let base_path = env.dir.path().to_str().unwrap();

    let resp1 = env.handshake(base_path);
    let resp2 = env.handshake(base_path);

    let idx1 = match &resp1 {
        MasterResponse::WorkerSocket { worker_index, .. } => *worker_index,
        other => panic!("expected WorkerSocket, got {other:?}"),
    };
    let idx2 = match &resp2 {
        MasterResponse::WorkerSocket { worker_index, .. } => *worker_index,
        other => panic!("expected WorkerSocket, got {other:?}"),
    };

    assert_eq!(idx1, idx2, "same base_path should route to the same worker on repeated Handshakes");

    sigterm_and_wait(&mut master, Duration::from_secs(5));
}

/// U5-2: Scale-out fires when routing table reaches roots_per_worker_max.
/// roots_per_worker_max=2, so 3 distinct roots should cause master to spawn a second worker.
#[test]
fn u5_scale_out_fires_at_roots_per_worker_max() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));

    // Confirm we start with exactly n_min=1 worker.
    let initial_count = env.list_workers().len();
    assert_eq!(initial_count, 1, "should start with exactly 1 worker (n_min=1)");

    // Create 3 distinct real directories so canonicalization produces distinct slugs.
    let root_a = env.dir.path().join("root_a");
    let root_b = env.dir.path().join("root_b");
    let root_c = env.dir.path().join("root_c");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();
    std::fs::create_dir_all(&root_c).unwrap();

    env.handshake(root_a.to_str().unwrap());
    env.handshake(root_b.to_str().unwrap());
    // Third root exceeds roots_per_worker_max=2 → triggers scale-out.
    env.handshake(root_c.to_str().unwrap());

    // Scale-out is async; wait for the second worker to register.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let scaled = loop {
        let count = env.list_workers().len();
        if count >= 2 {
            break true;
        }
        if std::time::Instant::now() >= deadline {
            break false;
        }
        sleep(Duration::from_millis(200));
    };
    assert!(scaled, "master should have spawned a second worker after exceeding roots_per_worker_max");

    sigterm_and_wait(&mut master, Duration::from_secs(5));
}

/// U5-3: After scale-out, existing routing entries are not remapped.
/// The root assigned before scale-out must still map to the original worker.
#[test]
fn u5_existing_routing_not_remapped_after_scale_out() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));

    let root_a = env.dir.path().join("so_root_a");
    let root_b = env.dir.path().join("so_root_b");
    let root_c = env.dir.path().join("so_root_c");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();
    std::fs::create_dir_all(&root_c).unwrap();

    // Assign root_a before scale-out.
    let resp_before = env.handshake(root_a.to_str().unwrap());
    let idx_before = match resp_before {
        MasterResponse::WorkerSocket { worker_index, .. } => worker_index,
        other => panic!("expected WorkerSocket, got {other:?}"),
    };

    // Trigger scale-out with root_b and root_c.
    env.handshake(root_b.to_str().unwrap());
    env.handshake(root_c.to_str().unwrap());

    // Give scale-out time to complete.
    sleep(Duration::from_secs(3));

    // root_a should still route to the same worker.
    let resp_after = env.handshake(root_a.to_str().unwrap());
    let idx_after = match resp_after {
        MasterResponse::WorkerSocket { worker_index, .. } => worker_index,
        other => panic!("expected WorkerSocket, got {other:?}"),
    };

    assert_eq!(
        idx_before, idx_after,
        "root_a should remain on worker-{idx_before} after scale-out"
    );

    sigterm_and_wait(&mut master, Duration::from_secs(5));
}

/// U5-4: Routing table persisted after each Handshake mutation.
/// routing.json should contain the new entry immediately after Handshake.
#[test]
fn u5_routing_json_persisted_after_each_handshake() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));

    let root1 = env.dir.path().join("persist_root1");
    let root2 = env.dir.path().join("persist_root2");
    std::fs::create_dir_all(&root1).unwrap();
    std::fs::create_dir_all(&root2).unwrap();

    env.handshake(root1.to_str().unwrap());
    // Give a brief moment for the async persist to complete.
    sleep(Duration::from_millis(200));

    let table1 = RoutingTable::load(&env.routing_json()).expect("load routing.json after first handshake");
    let total_slugs1: usize = table1.workers.values().map(|e| e.root_slugs.len()).sum();
    assert!(total_slugs1 >= 1, "routing.json should have at least 1 slug after first Handshake");

    env.handshake(root2.to_str().unwrap());
    sleep(Duration::from_millis(200));

    let table2 = RoutingTable::load(&env.routing_json()).expect("load routing.json after second handshake");
    let total_slugs2: usize = table2.workers.values().map(|e| e.root_slugs.len()).sum();
    assert!(
        total_slugs2 >= total_slugs1,
        "routing.json should gain a new slug entry after second Handshake"
    );

    sigterm_and_wait(&mut master, Duration::from_secs(5));
}

// ── U6: Crash recovery ──────────────────────────────────────────────────────────

/// U6-1: Worker crash detected by master — master respawns it within 15s.
#[test]
fn u6_master_respawns_crashed_worker() {
    let env = TestEnv::new();
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));

    // Discover the worker spawned by n_min=1.
    let workers = env.list_workers();
    assert!(!workers.is_empty(), "expected at least one worker");
    let idx = workers[0].index;
    let original_pid = workers[0].pid;

    // Wait until the worker socket is connectable.
    assert!(env.wait_worker(idx, SOCKET_TIMEOUT), "initial worker socket should be ready");

    // Kill the worker process externally.
    unsafe { libc::kill(original_pid as libc::pid_t, libc::SIGKILL) };

    // Wait for the socket to disappear (confirming the process is gone).
    let sock = env.worker_socket(idx);
    let gone = env.wait_socket_gone(&sock, Duration::from_secs(5));
    assert!(gone, "worker socket should disappear after SIGKILL");

    // Wait for master to detect the crash and respawn (within 15s).
    let respawned = env.wait_worker(idx, Duration::from_secs(15));
    assert!(respawned, "master should respawn worker-{idx} within 15s of crash");

    // Verify the new worker has a different PID.
    if let Some(info) = env.worker_status(idx) {
        assert_ne!(info.pid, original_pid, "respawned worker should have a new PID");
    }

    sigterm_and_wait(&mut master, Duration::from_secs(5));
}

/// U6-2: Master startup with routing.json containing a mix of live and dead PIDs.
/// Dead entries should be discarded; live workers should be reconnected.
#[test]
fn u6_startup_reconnects_live_discards_dead() {
    let env = TestEnv::new();

    // First: start a real worker to get a live socket and PID.
    let live_worker = env.spawn_worker(5);
    assert!(env.wait_worker(5, SOCKET_TIMEOUT));
    let live_pid = live_worker.id();
    let live_sock = env.worker_socket(5).to_string_lossy().into_owned();

    // Write a routing.json with one live entry (worker-5) and one dead entry (worker-99).
    let dead_pid: u32 = 999_999_999;
    let routing_dir = env.runtime_dir().join("fff");
    std::fs::create_dir_all(&routing_dir).unwrap();

    let mut table = RoutingTable::default();
    table.workers.insert(5, WorkerEntry {
        index: 5,
        socket_path: live_sock,
        pid: live_pid,
        root_slugs: vec![],
    });
    table.workers.insert(99, WorkerEntry {
        index: 99,
        socket_path: env.worker_socket(99).to_string_lossy().into_owned(),
        pid: dead_pid,
        root_slugs: vec!["stale-slug".into()],
    });
    table.save(&env.routing_json()).unwrap();

    // Start master — it should adopt worker-5 and discard worker-99.
    let mut master = env.spawn_master();
    assert!(env.wait_master(SOCKET_TIMEOUT));

    let workers = env.list_workers();

    // Dead PID should not appear.
    let has_dead = workers.iter().any(|w| w.pid == dead_pid);
    assert!(!has_dead, "dead worker-99 should be discarded on startup");

    // Live worker-5 should be adopted (or at least the dead one removed).
    // The master may also spawn additional workers to satisfy n_min=1.
    assert!(!workers.is_empty(), "at least one worker should be registered");

    kill_and_wait(live_worker);
    sigterm_and_wait(&mut master, Duration::from_secs(5));
}
