use std::path::{Path, PathBuf};

/// Socket path for a given project root:
/// `<cache_dir>/fff/sockets/<blake3hex(canonical_base_path)>.sock`
pub fn socket_path(base_path: &Path) -> PathBuf {
    let hash = path_hash(base_path);
    cache_dir().join("fff").join("sockets").join(format!("{hash}.sock"))
}

/// Lockfile path for a given project root:
/// `<cache_dir>/fff/locks/<blake3hex(canonical_base_path)>.lock`
pub fn lockfile_path(base_path: &Path) -> PathBuf {
    let hash = path_hash(base_path);
    cache_dir().join("fff").join("locks").join(format!("{hash}.lock"))
}

/// Unix domain sockets have a path limit of 104 bytes on macOS (SUN_LEN).
/// A full blake3 hex (64 chars) + ~/.../Library/Caches/fff/sockets/ prefix
/// exceeds that. Use the first 16 hex chars (8 bytes / 64 bits) — sufficient
/// for collision-resistant local disambiguation.
fn path_hash(base_path: &Path) -> String {
    let bytes = base_path.as_os_str().as_encoded_bytes();
    let hash = blake3::hash(bytes);
    hash.to_hex()[..16].to_string()
}

/// Resolves the XDG cache dir with the correct fallback for macOS (which does
/// not set `$XDG_CACHE_HOME` by default). `dirs::cache_dir()` encodes this:
/// `$XDG_CACHE_HOME` if set, else `$HOME/.cache` on Linux and
/// `$HOME/Library/Caches` on macOS.
fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".cache")
    })
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
