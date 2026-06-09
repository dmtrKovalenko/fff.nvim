//! Crash recovery — re-run the two-phase handshake after a broken connection.
//!
//! Primary path: up to MAX_ATTEMPTS retries of the full master+worker handshake
//! (master is re-spawned automatically inside `EngineClient::connect` if down).
//!
//! R2 fallback: if all master+worker attempts fail, try `connect_legacy` to
//! reach a per-root singleton engine directly — useful when the master is
//! unavailable but the user still has a legacy `fff-engine --base-path` running.

use std::path::Path;
use std::time::Duration;

use fff_ipc::IpcError;

use crate::client::EngineClient;

const BACKOFF_BASE_MS: u64 = 100;
const MAX_ATTEMPTS: u32 = 3;

/// Recover from a broken connection.
///
/// Retries the master+worker handshake up to MAX_ATTEMPTS times, then falls
/// back to a direct legacy per-root socket connection (R2 resilience).
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

    // R2: master+worker path exhausted — try legacy per-root singleton.
    match EngineClient::connect_legacy(base_path) {
        Ok(client) => {
            tracing::info!(
                "R2 fallback: connected to legacy per-root singleton for {}",
                base_path.display()
            );
            return Ok(client);
        }
        Err(e) => tracing::warn!("R2 legacy fallback unavailable: {e}"),
    }

    Err(last_err)
}
