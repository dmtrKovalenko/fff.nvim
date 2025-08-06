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
    use std::path::Path;
    #[test]
    fn test_calculate_distance_penalty() {
        {
            let other_path = Path::new("path").join("to").join("file.txt");
            assert_eq!(
                calculate_distance_penalty(None, other_path.to_str().unwrap()),
                0
            );
        }
        {
            let base_path = Path::new("path").join("to").join("current");
            let current_path = base_path.join("file.txt");
            let other_path = base_path.join("other.txt");
            assert_eq!(
                calculate_distance_penalty(
                    Some(current_path.to_str().unwrap()),
                    other_path.to_str().unwrap()
                ),
                0
            );
        }
        {
            let base_path = Path::new("path").join("to");
            let current_path = base_path.join("current").join("file.txt");
            let other_path = base_path.join("file.txt");
            assert_eq!(
                calculate_distance_penalty(
                    Some(current_path.to_str().unwrap()),
                    other_path.to_str().unwrap()
                ),
                -2
            );
        }
        {
            let base_path = Path::new("path").join("to");
            let current_path = base_path.join("current").join("file.txt");
            let other_path = base_path.join("other").join("file.txt");
            assert_eq!(
                calculate_distance_penalty(
                    Some(current_path.to_str().unwrap()),
                    other_path.to_str().unwrap()
                ),
                -4
            );
        }
        {
            let base_path = Path::new("path").join("to");
            let current_path = base_path.join("current").join("file.txt");
            let other_path = base_path.join("another").join("dir").join("file.txt");
            assert_eq!(
                calculate_distance_penalty(
                    Some(current_path.to_str().unwrap()),
                    other_path.to_str().unwrap()
                ),
                -6
            );
        }
        {
            let current_path = Path::new("a")
                .join("b")
                .join("c")
                .join("d")
                .join("file.txt");
            let other_path = Path::new("x")
                .join("y")
                .join("z")
                .join("w")
                .join("file.txt");
            assert_eq!(
                calculate_distance_penalty(
                    Some(current_path.to_str().unwrap()),
                    other_path.to_str().unwrap()
                ),
                -16
            );
        }
        {
            let current_path = Path::new("file1.txt").to_str().unwrap();
            let other_path = Path::new("file2.txt").to_str().unwrap();
            assert_eq!(
                calculate_distance_penalty(Some(current_path), other_path),
                0
            );
        }
    }
}
