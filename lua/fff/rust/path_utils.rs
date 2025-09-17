pub fn calculate_distance_penalty(current_file: Option<&str>, candidate_path: &str) -> i32 {
    let Some(ref current_path) = current_file else {
        return 0; // No penalty if no current file
    };

    let current_dir = if let Some(parent) = std::path::Path::new(current_path).parent() {
        parent.to_string_lossy().to_string()
    } else {
        String::new()
    };

    let candidate_dir = if let Some(parent) = std::path::Path::new(candidate_path).parent() {
        parent.to_string_lossy().to_string()
    } else {
        String::new()
    };

    if current_dir == candidate_dir {
        return 0; // Same directory, no penalty
    }

    let current_parts: Vec<&str> = current_dir
        .split(std::path::MAIN_SEPARATOR)
        .filter(|s| !s.is_empty())
        .collect();
    let candidate_parts: Vec<&str> = candidate_dir
        .split(std::path::MAIN_SEPARATOR)
        .filter(|s| !s.is_empty())
        .collect();

    let common_len = current_parts
        .iter()
        .zip(candidate_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let current_depth_from_common = current_parts.len() - common_len;

    if current_depth_from_common == 0 {
        return 0; // Current file is at the common ancestor level
    }

    let penalty = -(current_depth_from_common as i32);

    penalty.max(-20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(target_family = "windows"))]
    fn test_calculate_distance_penalty() {
        assert_eq!(
            calculate_distance_penalty(None, "examples/user/test/mod.rs"),
            0
        );
        // Same directory
        assert_eq!(
            calculate_distance_penalty(
                Some("examples/user/test/main.rs"),
                "examples/user/test/mod.rs"
            ),
            0
        );
        //
        // One level apart
        assert_eq!(
            calculate_distance_penalty(
                Some("examples/user/test/subdir/file.rs"),
                "examples/user/test/mod.rs"
            ),
            -1
        );
        //
        // Different subdirectories (same parent)
        assert_eq!(
            calculate_distance_penalty(
                Some("examples/user/test/dir1/file.rs"),
                "examples/user/test/dir2/mod.rs"
            ),
            -1
        );

        assert_eq!(
            calculate_distance_penalty(
                Some("examples/audio-announce/src/lib/audio-announce.rs"),
                "examples/audio-announce/src/main.rs"
            ),
            -1
        );

        assert_eq!(
            calculate_distance_penalty(
                Some("examples/audio-announce/src/audio-announce.rs"),
                "examples/pixel/src/main.rs"
            ),
            -2
        );

        // Root level files
        assert_eq!(calculate_distance_penalty(Some("main.rs"), "lib.rs"), 0);
    }

    #[test]
    #[cfg(target_family = "windows")]
    fn distance_penalty_works_on_windows() {
        assert_eq!(
            calculate_distance_penalty(None, "examples\\user\\test\\mod.rs"),
            0
        );
        // Same directory
        assert_eq!(
            calculate_distance_penalty(
                Some("examples\\user\\test\\main.rs"),
                "examples\\user\\test\\mod.rs"
            ),
            0
        );
        //
        // One level apart
        assert_eq!(
            calculate_distance_penalty(
                Some("examples\\user\\test\\subdir\\file.rs"),
                "examples\\user\\test\\mod.rs"
            ),
            -1
        );
    }
}
