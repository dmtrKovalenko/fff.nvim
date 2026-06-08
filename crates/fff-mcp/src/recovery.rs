//! Crash recovery — re-run the two-phase handshake after a broken connection.
//!
//! When a worker socket breaks (BrokenPipe, ECONNREFUSED), `respawn` calls
//! `EngineClient::connect` which re-runs the full master handshake. The master
//! is re-spawned automatically if it is also down (ensured inside `connect`).

use std::path::Path;
use std::time::Duration;

use fff_ipc::IpcError;

use crate::client::EngineClient;

const BACKOFF_BASE_MS: u64 = 100;
const MAX_ATTEMPTS: u32 = 3;

/// Recover from a broken connection by re-running the two-phase handshake.
pub fn respawn(base_path: &Path) -> Result<EngineClient, IpcError> {
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
                tracing::warn!("recovery attempt {}/{MAX_ATTEMPTS}: {last_err}", attempt + 1);
            }
        }
    }
    Err(last_err)
}
