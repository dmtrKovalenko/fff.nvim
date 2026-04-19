use std::path::{Path, PathBuf};

#[cfg(windows)]
pub fn canonicalize(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

#[cfg(not(windows))]
pub fn canonicalize(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

#[cfg(windows)]
pub fn expand_tilde(path: &str) -> PathBuf {
    return PathBuf::from(path);
}

#[cfg(not(windows))]
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/")
        && let Some(home_dir) = dirs::home_dir()
    {
        return home_dir.join(stripped);
    }

    PathBuf::from(path)
}

/// Calculate distance penalty based on directory proximity.
/// Returns a negative penalty score based on how far the candidate is from the current file.
///
/// `candidate_dir` is the directory portion of the candidate path (e.g. `"src/components/"`).
/// It may have a trailing `/` which is stripped internally.
///
/// Zero-allocation: walks both directory part iterators in lockstep.
pub fn calculate_distance_penalty(current_file: Option<&str>, candidate_dir: &str) -> i32 {
    let Some(current_path) = current_file else {
        return 0;
    };

    // Extract the directory from current_file (everything before the last '/')
    let current_dir = match current_path.rfind('/') {
        Some(pos) => &current_path[..pos],
        None => "",
    };

    // Strip trailing separator from candidate_dir
    let candidate_dir = candidate_dir.strip_suffix('/').unwrap_or(candidate_dir);

    if current_dir == candidate_dir {
        return 0;
    }

    // Count components and common prefix length by walking iterators in lockstep.
    // No Vec allocation — just iterate and count.
    let mut current_parts = current_dir.split('/').filter(|s| !s.is_empty());
    let mut candidate_parts = candidate_dir.split('/').filter(|s| !s.is_empty());

    let mut common_len = 0usize;
    let mut current_total = 0usize;

    loop {
        match (current_parts.next(), candidate_parts.next()) {
            (Some(a), Some(b)) => {
                current_total += 1;
                if a == b {
                    common_len += 1;
                } else {
                    // Diverged — count remaining current parts
                    current_total += current_parts.count();
                    break;
                }
            }
            (Some(_), None) => {
                // current is deeper
                current_total += 1 + current_parts.count();
                break;
            }
            (None, _) => {
                // current is at or above candidate
                break;
            }
        }
    }

    let depth_from_common = current_total - common_len;
    if depth_from_common == 0 {
        return 0;
    }

    (-(depth_from_common as i32)).max(-20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(target_family = "windows"))]
    fn test_calculate_distance_penalty() {
        // candidate_dir is now just the directory portion (with or without trailing /)
        assert_eq!(calculate_distance_penalty(None, "examples/user/test/"), 0);
        // Same directory
        assert_eq!(
            calculate_distance_penalty(Some("examples/user/test/main.rs"), "examples/user/test/"),
            0
        );
        //
        // One level apart
        assert_eq!(
            calculate_distance_penalty(
                Some("examples/user/test/subdir/file.rs"),
                "examples/user/test/"
            ),
            -1
        );
        //
        // Different subdirectories (same parent)
        assert_eq!(
            calculate_distance_penalty(
                Some("examples/user/test/dir1/file.rs"),
                "examples/user/test/dir2/"
            ),
            -1
        );

        assert_eq!(
            calculate_distance_penalty(
                Some("examples/audio-announce/src/lib/audio-announce.rs"),
                "examples/audio-announce/src/"
            ),
            -1
        );

        assert_eq!(
            calculate_distance_penalty(
                Some("examples/audio-announce/src/audio-announce.rs"),
                "examples/pixel/src/"
            ),
            -2
        );

        // Root level files (empty dir)
        assert_eq!(calculate_distance_penalty(Some("main.rs"), ""), 0);
    }

    #[test]
    #[cfg(target_family = "windows")]
    fn distance_penalty_works_on_windows() {
        assert_eq!(
            calculate_distance_penalty(None, "examples\\user\\test\\"),
            0
        );
        // Same directory
        assert_eq!(
            calculate_distance_penalty(
                Some("examples\\user\\test\\main.rs"),
                "examples\\user\\test\\"
            ),
            0
        );
        //
        // One level apart
        assert_eq!(
            calculate_distance_penalty(
                Some("examples\\user\\test\\subdir\\file.rs"),
                "examples\\user\\test\\"
            ),
            -1
        );
    }
}
