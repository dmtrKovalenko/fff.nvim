//! Synchronous IPC client for fff-engine — two-phase connect via master.
//!
//! Phase 1: connect to master socket, send Handshake{base_path}, receive
//! WorkerSocket{path, worker_index}.
//! Phase 2: connect to the worker socket, send Connect{base_path}, wait for Ack.
//! All subsequent search traffic uses the direct worker connection.

use std::io::{BufReader, BufWriter};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use fff_ipc::types::{MasterRequest, MasterResponse, SearchRequest, SearchResponse};
use fff_ipc::{lockfile, master_lockfile_path, master_socket_path, IpcError};
use fff_ipc::{read_message_sync, write_message_sync};

const SPAWN_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub struct EngineClient {
    reader: BufReader<UnixStream>,
    writer: BufWriter<UnixStream>,
    /// Stored for reconnect — passed back on two-phase re-handshake.
    pub base_path: PathBuf,
}

impl EngineClient {
    /// Connect to the fff-engine for `base_path` via master two-phase handshake.
    ///
    /// If master is not running, spawns `fff-engine --master` first.
    /// Falls back to the legacy singleton path when master spawn fails and a
    /// per-root socket exists (backwards compatibility).
    pub fn connect(base_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        // Ensure master is running; spawn if absent.
        ensure_master_running()?;

        // Phase 1: handshake with master to get the worker socket path.
        let worker_socket = master_handshake(base_path)?;

        // Phase 2: connect to the worker and send Connect{base_path}.
        let stream = wait_and_connect(&worker_socket, SPAWN_TIMEOUT)?;
        let base_path_str = base_path.to_string_lossy().into_owned();

        let mut writer = BufWriter::new(stream.try_clone()?);
        let mut reader = BufReader::new(stream);

        write_message_sync(&mut writer, &SearchRequest::Connect { base_path: base_path_str.clone() })?;
        use std::io::Write;
        writer.flush().map_err(IpcError::Io)?;

        let connect_resp: SearchResponse = read_message_sync(&mut reader)?;
        match connect_resp {
            SearchResponse::Ack => {}
            SearchResponse::Error(e) => return Err(format!("worker Connect rejected: {e}").into()),
            other => return Err(format!("unexpected worker response: {other:?}").into()),
        }

        Ok(Self { reader, writer, base_path: base_path.to_path_buf() })
    }

    /// Re-run the two-phase handshake and return a fresh client. Used by recovery.
    pub fn reconnect(&self) -> Result<Self, Box<dyn std::error::Error>> {
        Self::connect(&self.base_path)
    }

    /// Send a search request, with transparent crash recovery.
    pub fn search_with_recovery(&mut self, req: &SearchRequest, base_path: &Path) -> SearchResponse {
        match self.search(req) {
            Ok(resp) => return resp,
            Err(e) => tracing::warn!("worker socket error: {e}; attempting recovery"),
        }

        // Re-run two-phase handshake to get a fresh worker connection.
        match crate::recovery::respawn(base_path) {
            Ok(new_client) => {
                *self = new_client;
                match self.search(req) {
                    Ok(resp) => resp,
                    Err(e) => SearchResponse::Error(format!("fff-engine unavailable after recovery: {e}")),
                }
            }
            Err(e) => SearchResponse::Error(format!("fff-engine recovery failed: {e}")),
        }
    }

    /// Low-level send with no retry.
    pub fn search(&mut self, req: &SearchRequest) -> Result<SearchResponse, IpcError> {
        write_message_sync(&mut self.writer, req)?;
        use std::io::Write;
        self.writer.flush().map_err(IpcError::Io)?;
        read_message_sync(&mut self.reader)
    }

    /// Hot-reload the daemon's log filter.
    pub fn set_log_level(&mut self, level: &str) -> Result<SearchResponse, IpcError> {
        self.search(&SearchRequest::SetLogLevel { level: level.to_owned() })
    }

    /// Fire-and-forget frecency write.
    #[allow(dead_code)]
    pub fn record_access(&mut self, path: &str) {
        let req = SearchRequest::RecordAccess { path: path.to_owned() };
        let _ = write_message_sync(&mut self.writer, &req);
        let _ = {
            use std::io::Write;
            self.writer.flush()
        };
    }

    /// Check daemon health.
    pub fn check_health(base_path: &Path) -> HealthStatus {
        let master = master_socket_path();
        if !master.exists() {
            // Fall back to legacy per-root socket check.
            let sock = fff_ipc::socket_path(base_path);
            if !sock.exists() {
                return HealthStatus::NotStarted(master);
            }
        }
        match Self::connect(base_path) {
            Ok(_) => HealthStatus::Ok,
            Err(e) => HealthStatus::ConnRefused(e.to_string()),
        }
    }
}

pub enum HealthStatus {
    Ok,
    NotStarted(std::path::PathBuf),
    ConnRefused(String),
}

/// Send a Handshake to the master and return the worker socket path.
fn master_handshake(base_path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let master = master_socket_path();
    let stream = UnixStream::connect(&master)
        .map_err(|e| format!("cannot connect to master socket {}: {e}", master.display()))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;

    let mut writer = BufWriter::new(stream.try_clone()?);
    let mut reader = BufReader::new(stream);

    let base_str = base_path.to_string_lossy().into_owned();
    write_message_sync(&mut writer, &MasterRequest::Handshake { base_path: base_str })?;
    use std::io::Write;
    writer.flush().map_err(IpcError::Io)?;

    let resp: MasterResponse = read_message_sync(&mut reader)?;
    match resp {
        MasterResponse::WorkerSocket { path, .. } => Ok(PathBuf::from(path)),
        MasterResponse::Error(e) => Err(format!("master handshake error: {e}").into()),
        other => Err(format!("unexpected master response: {other:?}").into()),
    }
}

/// Ensure `fff-engine --master` is running, spawning it if absent.
/// Uses an O_CREAT|O_EXCL race so only one spawner wins.
fn ensure_master_running() -> Result<(), Box<dyn std::error::Error>> {
    let master = master_socket_path();

    // Fast path: master socket already exists and accepts connections.
    if master.exists() && UnixStream::connect(&master).is_ok() {
        return Ok(());
    }

    let lockfile = master_lockfile_path();

    // Check whether a live master holds the lockfile (slow start).
    if lockfile.exists() && !lockfile::is_stale(&lockfile) {
        // Master is alive but socket not ready yet — wait.
        return wait_for_socket(&master, SPAWN_TIMEOUT);
    }

    // Race to spawn master: O_CREAT|O_EXCL via create_new.
    use std::fs::OpenOptions;
    match OpenOptions::new().write(true).create_new(true).open(&lockfile) {
        Ok(_) => {
            // We won — clear the temp lockfile; fff-engine --master will own it.
            let _ = std::fs::remove_file(&lockfile);
        }
        Err(_) => {
            // Someone else is spawning; wait for the socket.
            return wait_for_socket(&master, SPAWN_TIMEOUT);
        }
    }

    let engine_bin = find_engine_bin();
    if let Some(parent) = master.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let child = Command::new(&engine_bin)
        .arg("--master")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", engine_bin.display()))?;

    tracing::info!("spawned fff-engine --master pid={}", child.id());

    wait_for_socket(&master, SPAWN_TIMEOUT)?;
    Ok(())
}

/// Wait until `path` exists by polling, up to `timeout`.
fn wait_for_socket(path: &Path, timeout: Duration) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if path.exists() {
            return Ok(());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Err(format!("timed out waiting for socket at {} ({}s)", path.display(), timeout.as_secs()).into())
}

/// Wait for `path` to exist then connect.
fn wait_and_connect(path: &Path, timeout: Duration) -> Result<UnixStream, Box<dyn std::error::Error>> {
    wait_for_socket(path, timeout)?;
    UnixStream::connect(path)
        .map_err(|e| format!("failed to connect to worker socket {}: {e}", path.display()).into())
}

fn find_engine_bin() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("fff-engine")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("fff-engine"))
}
