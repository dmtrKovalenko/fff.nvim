//! Crash recovery for the fff-engine connection.
//!
//! When `search()` fails with a broken pipe or ECONNREFUSED, `respawn`
//! determines whether the daemon is still alive (slow start) or dead, then
//! acts accordingly:
//!
//! - **Live PID** (`kill(pid, 0)` succeeds): daemon is starting slowly.
//!   Wait with exponential backoff (100 ms → 200 ms → 400 ms, max 3 attempts).
//! - **Dead PID** or missing lockfile: delete stale lockfile and socket, then
//!   re-run spawn-if-absent via `EngineClient::connect`.

use std::path::Path;
use std::time::Duration;

use fff_ipc::{lockfile, IpcError};

use crate::client::EngineClient;

const BACKOFF_BASE_MS: u64 = 100;
const MAX_ATTEMPTS: u32 = 3;

/// Attempt to recover from a broken engine connection.
///
/// Returns a fresh connected `EngineClient` or the last `IpcError` after
/// `MAX_ATTEMPTS` failed attempts.
pub fn respawn(base_path: &Path) -> Result<EngineClient, IpcError> {
    let lockfile = fff_ipc::lockfile_path(base_path);
    let socket = fff_ipc::socket_path(base_path);

    // Check whether the daemon is still alive using the PID in the lockfile.
    match lockfile::read(&lockfile) {
        Some(lock) if lock.is_alive() => {
            let pid = lock.pid;
            tracing::info!("fff-engine PID {pid} is still alive — waiting for it to bind");
            // Daemon is alive but socket not ready yet. Wait with backoff.
            for attempt in 0..MAX_ATTEMPTS {
                let delay = Duration::from_millis(BACKOFF_BASE_MS * (1 << attempt));
                std::thread::sleep(delay);
                if let Ok(client) = EngineClient::connect(base_path) {
                    return Ok(client);
                }
            }
            Err(IpcError::Io(std::io::Error::other(format!(
                "fff-engine PID {pid} is alive but did not accept connections after {MAX_ATTEMPTS} attempts"
            ))))
        }
        _ => {
            // PID is dead or lockfile absent/unreadable — stale state.
            tracing::info!("fff-engine appears dead; removing stale lockfile and socket");
            let _ = std::fs::remove_file(&lockfile);
            let _ = std::fs::remove_file(&socket);

            // Re-run spawn-if-absent with backoff in case of a race.
            let mut last_err = IpcError::Io(std::io::Error::other("no attempts made"));
            for attempt in 0..MAX_ATTEMPTS {
                if attempt > 0 {
                    let delay = Duration::from_millis(BACKOFF_BASE_MS * (1 << (attempt - 1)));
                    std::thread::sleep(delay);
                }
                match EngineClient::connect(base_path) {
                    Ok(client) => return Ok(client),
                    Err(e) => {
                        last_err = IpcError::Io(std::io::Error::other(e.to_string()));
                        tracing::warn!("Respawn attempt {}/{MAX_ATTEMPTS} failed: {last_err}", attempt + 1);
                    }
                }
            }
            Err(last_err)
        }
    }
}

