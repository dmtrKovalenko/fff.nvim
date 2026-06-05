//! Lockfile format shared by fff-engine (writer), fff-mcp (reader), and
//! fff-ctl (reader). On-disk layout:
//!
//! ```text
//! <pid>
//! <absolute base-path>
//! ```
//!
//! The base-path line is optional for backwards compatibility with v0.10.0
//! lockfiles, which only contained the PID.

use std::path::{Path, PathBuf};

/// Parsed lockfile contents.
#[derive(Debug, Clone)]
pub struct Lockfile {
    pub pid: u32,
    /// Absolute base-path the daemon was started with. `None` for legacy
    /// single-line lockfiles produced by older daemons.
    pub base_path: Option<PathBuf>,
}

impl Lockfile {
    /// Probe whether the PID is still alive. Uses `kill(pid, 0)`.
    pub fn is_alive(&self) -> bool {
        // SAFETY: signal 0 only probes existence, never delivers anything.
        unsafe { libc::kill(self.pid as libc::pid_t, 0) == 0 }
    }
}

/// Serialise lockfile contents for `std::fs::write`.
pub fn format_contents(pid: u32, base_path: &Path) -> String {
    format!("{pid}\n{}\n", base_path.display())
}

/// Read and parse a lockfile. Returns `None` if the file is missing,
/// unreadable, or its first line is not a valid PID.
pub fn read(path: &Path) -> Option<Lockfile> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut lines = content.lines();
    let pid: u32 = lines.next()?.trim().parse().ok()?;
    let base_path = lines
        .next()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    Some(Lockfile { pid, base_path })
}

/// Returns true when the lockfile is missing, unparseable, or owned by a
/// dead PID. Live daemons return false. Used to decide whether a stale
/// lockfile can be safely removed before a fresh acquire.
pub fn is_stale(path: &Path) -> bool {
    match read(path) {
        None => true,
        Some(lock) => !lock.is_alive(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn reads_pid_and_base_path() {
        let dir = tmp();
        let path = dir.path().join("test.lock");
        std::fs::write(&path, "12345\n/some/project\n").unwrap();

        let lock = read(&path).expect("parses");
        assert_eq!(lock.pid, 12345);
        assert_eq!(lock.base_path, Some(PathBuf::from("/some/project")));
    }

    #[test]
    fn reads_legacy_single_line_lockfile() {
        let dir = tmp();
        let path = dir.path().join("legacy.lock");
        std::fs::write(&path, "12345").unwrap();

        let lock = read(&path).expect("parses");
        assert_eq!(lock.pid, 12345);
        assert_eq!(lock.base_path, None);
    }

    #[test]
    fn missing_file_is_stale() {
        let dir = tmp();
        assert!(is_stale(&dir.path().join("missing.lock")));
    }

    #[test]
    fn unparseable_file_is_stale() {
        let dir = tmp();
        let path = dir.path().join("garbage.lock");
        std::fs::write(&path, "not a pid").unwrap();
        assert!(is_stale(&path));
    }

    #[test]
    fn live_self_pid_not_stale() {
        let dir = tmp();
        let path = dir.path().join("self.lock");
        std::fs::write(&path, format_contents(std::process::id(), Path::new("/tmp"))).unwrap();
        assert!(!is_stale(&path));
    }

    #[test]
    fn dead_pid_is_stale() {
        let dir = tmp();
        let path = dir.path().join("dead.lock");
        // PID 0 is reserved by the kernel and never owned by a user process.
        // Use a deliberately-impossible value to simulate a dead daemon.
        std::fs::write(&path, "999999999\n/tmp\n").unwrap();
        assert!(is_stale(&path));
    }
}
