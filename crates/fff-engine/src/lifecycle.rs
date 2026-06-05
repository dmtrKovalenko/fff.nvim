use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// RAII guard that removes the lockfile on drop.
pub struct LockfileGuard {
    path: PathBuf,
}

impl Drop for LockfileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Attempt to acquire the lockfile exclusively.
///
/// Uses `O_CREAT | O_EXCL` semantics via `OpenOptions::create_new`. If the
/// lockfile already exists but the PID inside it is dead (crashed or killed
/// without cleanup), the stale file is removed and the acquire is retried
/// once. Returns `Err` only when a live daemon holds the lock.
pub fn acquire_lockfile(lockfile_path: &Path) -> Result<LockfileGuard, Box<dyn std::error::Error>> {
    if let Some(parent) = lockfile_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    match try_create_lockfile(lockfile_path) {
        Ok(()) => {}
        Err(_) => {
            // Lockfile exists. Check whether the owning process is still alive.
            if is_lockfile_stale(lockfile_path) {
                eprintln!("fff-engine: removing stale lockfile (previous process exited uncleanly)");
                let _ = std::fs::remove_file(lockfile_path);
                // Retry once — if this fails a live daemon raced us.
                try_create_lockfile(lockfile_path).map_err(|_| {
                    "another fff-engine daemon is already running for this project root"
                })?;
            } else {
                return Err("another fff-engine daemon is already running for this project root".into());
            }
        }
    }

    let pid = std::process::id();
    std::fs::write(lockfile_path, pid.to_string())?;

    Ok(LockfileGuard {
        path: lockfile_path.to_path_buf(),
    })
}

fn try_create_lockfile(path: &Path) -> std::io::Result<()> {
    OpenOptions::new().write(true).create_new(true).open(path).map(|_| ())
}

fn is_lockfile_stale(path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else { return true };
    let Ok(pid) = content.trim().parse::<u32>() else { return true };
    // kill(pid, 0) — signal 0 probes existence without delivering anything.
    let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
    !alive
}

/// Poll until `socket_path` exists, returning `Ok` when it does.
///
/// Used by fff-mcp (U7) to wait for fff-engine to bind its socket.
#[allow(dead_code)]
///
/// Used by fff-mcp to wait for fff-engine to finish binding the socket
/// (binding IS the readiness signal — see U5 server.rs).
pub fn await_ready_signal(
    socket_path: &Path,
    timeout: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = std::time::Instant::now() + timeout;
    let poll_interval = Duration::from_millis(20);

    while std::time::Instant::now() < deadline {
        if socket_path.exists() {
            return Ok(());
        }
        std::thread::sleep(poll_interval);
    }

    Err(format!(
        "Timed out waiting for daemon socket at {}",
        socket_path.display()
    )
    .into())
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::time::Duration;
    use tempfile::TempDir;

    use super::*;

    fn tmp() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn acquire_creates_lockfile() {
        let dir = tmp();
        let lock = dir.path().join("test.lock");
        let _guard = acquire_lockfile(&lock).unwrap();
        assert!(lock.exists());
    }

    #[test]
    fn second_acquire_fails() {
        let dir = tmp();
        let lock = dir.path().join("test.lock");
        let _guard = acquire_lockfile(&lock).unwrap();
        assert!(acquire_lockfile(&lock).is_err());
    }

    #[test]
    fn guard_drop_removes_lockfile() {
        let dir = tmp();
        let lock = dir.path().join("test.lock");
        {
            let _guard = acquire_lockfile(&lock).unwrap();
            assert!(lock.exists());
        }
        assert!(!lock.exists());
    }

    #[test]
    fn await_ready_signal_succeeds_when_file_exists() {
        let dir = tmp();
        let sock = dir.path().join("test.sock");
        File::create(&sock).unwrap();
        assert!(await_ready_signal(&sock, Duration::from_millis(100)).is_ok());
    }

    #[test]
    fn await_ready_signal_times_out() {
        let dir = tmp();
        let sock = dir.path().join("missing.sock");
        let result = await_ready_signal(&sock, Duration::from_millis(50));
        assert!(result.is_err());
    }
}
