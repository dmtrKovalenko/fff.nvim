//! Synchronous IPC client for fff-engine.
//!
//! Used by the tool handlers (which are sync functions within the rmcp server).
//! Spawn-if-absent logic runs the first time `connect` is called.

use std::io::{BufReader, BufWriter};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use fff_ipc::types::{SearchRequest, SearchResponse};
use fff_ipc::{lockfile_path, socket_path, IpcError};
use fff_ipc::{read_message_sync, write_message_sync};

/// How long to wait for fff-engine to bind its socket after spawning.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(10);
/// Poll interval while waiting for socket to appear.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub struct EngineClient {
    reader: BufReader<UnixStream>,
    writer: BufWriter<UnixStream>,
}

impl EngineClient {
    /// Connect to the fff-engine for `base_path`, spawning it if absent.
    pub fn connect(base_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let sock = socket_path(base_path);
        let lock = lockfile_path(base_path);

        // If socket already exists, try to connect directly.
        if sock.exists() && let Ok(stream) = UnixStream::connect(&sock) {
            return Self::from_stream(stream);
        }

        // Try to become the spawner via O_CREAT|O_EXCL on the lockfile.
        if let Some(parent) = lock.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let won_lock = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .is_ok();

        if won_lock {
            // We won the race: spawn fff-engine and write the child PID.
            let frecency_path = dirs::data_dir()
                .unwrap_or_else(|| {
                    dirs::home_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                        .join(".local/share")
                })
                .join("fff")
                .join("frecency");

            let child = Command::new("fff-engine")
                .arg("--base-path")
                .arg(base_path)
                .arg("--frecency-db")
                .arg(&frecency_path)
                .spawn()
                .map_err(|e| format!("Failed to spawn fff-engine: {e} (is fff-engine on PATH?)"))?;

            // Write the child PID so crash recovery can distinguish slow-start from dead.
            let _ = std::fs::write(&lock, child.id().to_string());

            tracing::info!("Spawned fff-engine PID={} for {}", child.id(), base_path.display());
        } else {
            tracing::info!("fff-engine already being spawned by another fff-mcp instance");
        }

        // Both winner and loser wait for the socket file to appear.
        wait_for_socket(&sock, SPAWN_TIMEOUT)?;

        let stream = UnixStream::connect(&sock)
            .map_err(|e| format!("Failed to connect to fff-engine socket: {e}"))?;

        Self::from_stream(stream)
    }

    fn from_stream(stream: UnixStream) -> Result<Self, Box<dyn std::error::Error>> {
        let reader = BufReader::new(stream.try_clone()?);
        let writer = BufWriter::new(stream);
        Ok(Self { reader, writer })
    }

    /// Send a search request and receive the response.
    ///
    /// Returns `Err` on socket failure. Caller (crash recovery) is responsible
    /// for reconnecting and retrying.
    pub fn search(&mut self, req: &SearchRequest) -> Result<SearchResponse, IpcError> {
        write_message_sync(&mut self.writer, req)?;
        use std::io::Write;
        self.writer.flush().map_err(IpcError::Io)?;
        read_message_sync(&mut self.reader)
    }

    /// Fire-and-forget frecency write. Sends RecordAccess and does NOT read a
    /// response. KTD-5: not called from tool handlers in this track.
    #[allow(dead_code)]
    pub fn record_access(&mut self, path: &str) {
        let req = SearchRequest::RecordAccess { path: path.to_owned() };
        let _ = write_message_sync(&mut self.writer, &req);
        let _ = {
            use std::io::Write;
            self.writer.flush()
        };
        // No response expected — see fire-and-forget semantics in KTD-5.
    }

    /// Check daemon health: attempt a fresh connection, return a human-readable
    /// status string for `--healthcheck`.
    pub fn check_health(base_path: &Path) -> HealthStatus {
        let sock = socket_path(base_path);
        if !sock.exists() {
            return HealthStatus::NotStarted(sock);
        }
        match UnixStream::connect(&sock) {
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

fn wait_for_socket(socket_path: &Path, timeout: Duration) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if socket_path.exists() {
            return Ok(());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Err(format!(
        "Timed out waiting for fff-engine socket at {} ({}s)",
        socket_path.display(),
        timeout.as_secs()
    )
    .into())
}
