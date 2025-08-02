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

    let current_parts: Vec<&str> = current_dir.split('/').filter(|s| !s.is_empty()).collect();
    let candidate_parts: Vec<&str> = candidate_dir.split('/').filter(|s| !s.is_empty()).collect();

    let common_len = current_parts
        .iter()
        .zip(candidate_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let current_depth_from_common = current_parts.len() - common_len;
    let candidate_depth_from_common = candidate_parts.len() - common_len;
    let total_distance = current_depth_from_common + candidate_depth_from_common;

    if total_distance == 0 {
        return 0; // Same path
    }

    let penalty = -(total_distance as i32 * 2);

    penalty.max(-20)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_calculate_distance_penalty() {
        assert_eq!(calculate_distance_penalty(None, "/path/to/file.txt"), 0);

        assert_eq!(
            calculate_distance_penalty(
                Some("/path/to/current/file.txt"),
                "/path/to/current/other.txt"
            ),
            0
        );

        assert_eq!(
            calculate_distance_penalty(Some("/path/to/current/file.txt"), "/path/to/file.txt"),
            -2
        );

        assert_eq!(
            calculate_distance_penalty(
                Some("/path/to/current/file.txt"),
                "/path/to/other/file.txt"
            ),
            -4
        );

        assert_eq!(
            calculate_distance_penalty(
                Some("/path/to/current/file.txt"),
                "/path/to/another/dir/file.txt"
            ),
            -6
        );

        assert_eq!(
            calculate_distance_penalty(Some("/a/b/c/d/file.txt"), "/x/y/z/w/file.txt"),
            -16
        );

        assert_eq!(
            calculate_distance_penalty(Some("/file1.txt"), "/file2.txt"),
            0
        );
    }
}
