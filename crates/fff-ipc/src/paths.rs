use std::path::{Path, PathBuf};

/// Socket path for a given project root:
/// `<cache_dir>/fff/sockets/<blake3hex(canonical_base_path)>.sock`
pub fn socket_path(base_path: &Path) -> PathBuf {
    let hash = base_path_slug(base_path);
    cache_dir().join("fff").join("sockets").join(format!("{hash}.sock"))
}

/// Lockfile path for a given project root:
/// `<cache_dir>/fff/locks/<blake3hex(canonical_base_path)>.lock`
pub fn lockfile_path(base_path: &Path) -> PathBuf {
    let hash = base_path_slug(base_path);
    cache_dir().join("fff").join("locks").join(format!("{hash}.lock"))
}

/// Log file path for a given project root:
/// `<cache_dir>/fff/logs/<blake3hex(canonical_base_path)>.log`
pub fn log_path(base_path: &Path) -> PathBuf {
    let hash = base_path_slug(base_path);
    cache_dir().join("fff").join("logs").join(format!("{hash}.log"))
}

/// Stable 16-hex-char slug for a project root. Used to derive per-base-path
/// paths (sockets, lockfiles, frecency DB) that don't collide across projects.
///
/// Canonicalize before hashing so that `.`, `./`, `/abs/path`, and
/// `/abs/path/` all produce the same hash. This ensures fff-engine (started
/// with `--base-path .`) and fff-mcp (which resolves the git workdir to an
/// absolute path) derive the same artefact paths.
///
/// Unix socket path limit on macOS is 104 bytes (SUN_LEN). Using 16 hex
/// chars (64-bit prefix) keeps the full path well under that limit.
pub fn base_path_slug(base_path: &Path) -> String {
    let canonical = std::fs::canonicalize(base_path)
        .unwrap_or_else(|_| base_path.to_path_buf());
    let bytes = canonical.as_os_str().as_encoded_bytes();
    let hash = blake3::hash(bytes);
    hash.to_hex()[..16].to_string()
}

/// XDG cache directory: `$XDG_CACHE_HOME` → `$HOME/.cache` → `dirs::cache_dir()` → `/tmp`.
///
/// Matches the XDG Base Directory Specification rather than macOS-canonical
/// `~/Library/Caches`. All fff cache artefacts (sockets, lockfiles, logs)
/// land here so they are consistent and easy to inspect on any platform.
pub fn xdg_cache_dir() -> PathBuf {
    if let Ok(v) = std::env::var("XDG_CACHE_HOME") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    // XDG spec default
    if let Some(home) = dirs::home_dir() {
        return home.join(".cache");
    }
    // Platform-canonical fallback (e.g. ~/Library/Caches on macOS)
    dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// XDG data directory: `$XDG_DATA_HOME` → `$HOME/.local/share` → `dirs::data_dir()` → `/tmp`.
pub fn xdg_data_dir() -> PathBuf {
    if let Ok(v) = std::env::var("XDG_DATA_HOME") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".local").join("share");
    }
    dirs::data_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn cache_dir() -> PathBuf {
    xdg_cache_dir()
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_under_sockets_dir() {
        let p = socket_path(Path::new("/some/project"));
        let components: Vec<_> = p.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        assert!(
            components.windows(2).any(|w| w[0] == "fff" && w[1] == "sockets"),
            "expected .../fff/sockets/... in {p:?}"
        );
    }

    #[test]
    fn socket_path_ends_with_sock() {
        let p = socket_path(Path::new("/some/project"));
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("sock"));
    }

    #[test]
    fn different_base_paths_give_distinct_sockets() {
        let a = socket_path(Path::new("/project/a"));
        let b = socket_path(Path::new("/project/b"));
        assert_ne!(a, b);
    }

    #[test]
    fn same_base_path_gives_stable_socket() {
        let a = socket_path(Path::new("/project/a"));
        let b = socket_path(Path::new("/project/a"));
        assert_eq!(a, b);
    }

    #[test]
    fn lockfile_under_locks_dir_with_lock_ext() {
        let p = lockfile_path(Path::new("/some/project"));
        let components: Vec<_> = p.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        assert!(
            components.windows(2).any(|w| w[0] == "fff" && w[1] == "locks"),
            "expected .../fff/locks/... in {p:?}"
        );
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("lock"));
    }
}
