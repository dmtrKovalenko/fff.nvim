use crate::error::{Error, Result};
use once_cell::sync::Lazy;
use std::borrow::Cow;
use std::path::Component;
use std::path::MAIN_SEPARATOR;
use std::{
    path::{Path, PathBuf},
    sync::RwLock,
};

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

#[derive(Clone, Copy, Default)]
pub enum PathShortenStrategy {
    #[default]
    MiddleNumber,
    Middle,
    End,
}

struct CacheEntry {
    shortened: String,
    max_size: usize,
}

struct PathCache {
    map: ahash::AHashMap<PathBuf, CacheEntry>,
    max_entries: usize,
}

impl PathCache {
    fn new(max_entries: usize) -> Self {
        Self {
            map: ahash::AHashMap::with_capacity(max_entries),
            max_entries,
        }
    }

    #[tracing::instrument(skip(self), fields(path = %path.display(), max_size))]
    fn get(&self, path: &Path, max_size: usize) -> Option<&str> {
        self.map.get(path).and_then(|entry| {
            // Only return cached value if max_size matches
            if entry.max_size == max_size {
                Some(entry.shortened.as_str())
            } else {
                None
            }
        })
    }

    fn insert(&mut self, path: PathBuf, shortened: String, max_size: usize) {
        // Simple eviction: clear half the cache when full
        if self.map.len() >= self.max_entries {
            let keys_to_remove: Vec<_> = self
                .map
                .keys()
                .take(self.max_entries / 2)
                .cloned()
                .collect();
            for key in keys_to_remove {
                self.map.remove(&key);
            }
        }
        self.map.insert(
            path,
            CacheEntry {
                shortened,
                max_size,
            },
        );
    }
}

// this is the amount of PATHS not entries
const DEFAULT_CACHE_SIZE: usize = 8192;

static PATH_SHORTEN_CACHE: Lazy<RwLock<PathCache>> =
    Lazy::new(|| RwLock::new(PathCache::new(DEFAULT_CACHE_SIZE)));

pub fn shorten_path(strategy: PathShortenStrategy, max_size: usize, path: &Path) -> Result<String> {
    {
        let cache = PATH_SHORTEN_CACHE
            .read()
            .map_err(|_| Error::AcquirePathCacheLock)?;
        if let Some(cached) = cache.get(path, max_size) {
            tracing::debug!("Cache hit for path '{}'", path.display());
            return Ok(cached.to_string());
        }
    }

    let shortened = strategy.shorten_path(path, max_size);
    {
        let mut cache = PATH_SHORTEN_CACHE
            .write()
            .map_err(|_| Error::AcquirePathCacheLock)?;
        cache.insert(path.to_path_buf(), shortened.clone(), max_size);
    }

    Ok(shortened)
}

impl TryFrom<mlua::Value> for PathShortenStrategy {
    type Error = mlua::Error;

    fn try_from(value: mlua::Value) -> std::result::Result<Self, Self::Error> {
        let strategy = match value {
            mlua::Value::String(ref s) => match s.to_str()?.as_ref() {
                "middle_number" => PathShortenStrategy::MiddleNumber,
                "middle" => PathShortenStrategy::Middle,
                "end" => PathShortenStrategy::End,
                _ => PathShortenStrategy::MiddleNumber,
            },
            _ => PathShortenStrategy::MiddleNumber,
        };

        Ok(strategy)
    }
}

impl PathShortenStrategy {
    pub fn shorten_path(&self, path: &Path, max_size: usize) -> String {
        const MIN_SMART_SHORTEN_SIZE: usize = 8;

        let sep = MAIN_SEPARATOR;

        let path_str = path.to_string_lossy();
        if path_str.len() <= max_size {
            return path_str.to_string();
        }

        // If max_size is too small for smart shortening, just truncate
        if max_size < MIN_SMART_SHORTEN_SIZE {
            return Self::truncate_str(&path_str, max_size);
        }

        let components: Vec<&str> = path
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => s.to_str(),
                _ => None,
            })
            .collect();

        if components.is_empty() {
            return path_str.to_string();
        }

        // For single component, just truncate it
        if components.len() == 1 {
            return Self::truncate_str(components[0], max_size);
        }

        match self {
            PathShortenStrategy::End => {
                // Simple truncation from the end
                let mut result = String::new();

                for (i, component) in components.iter().enumerate() {
                    let candidate = if i == 0 {
                        component.to_string()
                    } else {
                        format!("{}{}{}", result, sep, component)
                    };

                    if candidate.len() <= max_size {
                        result = candidate;
                    } else {
                        break;
                    }
                }

                // If even the first component is too long, truncate it
                if result.is_empty() && !components.is_empty() {
                    return components.first().map_or(String::new(), |component| {
                        let mut component = component.to_string();

                        component.truncate(max_size);
                        component
                    });
                }

                result
            }
            PathShortenStrategy::Middle | PathShortenStrategy::MiddleNumber => {
                let use_number = matches!(self, PathShortenStrategy::MiddleNumber);
                self.shorten_middle(&components, max_size, use_number, sep)
            }
        }
    }

    // rust doesn't have an ergonomic way to clone and truncate
    fn truncate_str(s: &str, max_len: usize) -> String {
        if max_len == 0 {
            return String::new();
        }

        let char_count = s.chars().count();
        if char_count <= max_len {
            return s.to_string();
        }

        // Just take the first max_len characters - no ".." suffix
        s.chars().take(max_len).collect()
    }

    fn shorten_middle(
        &self,
        components: &[&str],
        max_size: usize,
        use_number: bool,
        sep: char,
    ) -> String {
        let total = components.len();

        // For 2 components, just show both or truncate to fit
        if total <= 2 {
            let joined = components.join(&sep.to_string());
            if joined.len() <= max_size {
                return joined;
            }
            // Try to keep last intact, truncate first
            let last = components[total - 1];
            let available_for_first = max_size.saturating_sub(1 + last.len()); // sep + last
            if available_for_first > 0 && last.len() < max_size {
                let truncated = Self::truncate_str(components[0], available_for_first);
                let mut result = String::with_capacity(truncated.len() + 1 + last.len());
                result.push_str(&truncated);
                result.push(sep);
                result.push_str(last);
                return result;
            }
            // Last component alone exceeds max_size, must truncate it
            return Self::truncate_str(last, max_size);
        }

        let first = components[0];
        let last = components[total - 1];

        let initial_hidden = total - 2;
        let ellipsis = Self::make_ellipsis(initial_hidden, use_number);

        // Minimum pattern: first/.../last
        let min_overhead = 2 + ellipsis.len(); // two separators + ellipsis
        let min_content = first.len() + last.len();

        if min_content + min_overhead <= max_size {
            // We can fit first/.../last, now try to add more components
            return self.expand_middle(components, max_size, use_number, sep);
        }

        // Need to truncate to fit max_size
        // Priority: keep last intact if possible, truncate first, then truncate last if needed
        let needed_for_last = last.len() + 1 + ellipsis.len() + 1; // sep + ellipsis + sep + last
        if needed_for_last <= max_size {
            let available_for_first = max_size - needed_for_last;
            let truncated_first = Self::truncate_str(first, available_for_first);
            let ellipsis = Self::make_ellipsis(initial_hidden, use_number);
            // truncated_first + sep + ellipsis + sep + last
            let capacity = truncated_first.len() + 1 + ellipsis.len() + 1 + last.len();
            let mut result = String::with_capacity(capacity);
            result.push_str(&truncated_first);
            result.push(sep);
            result.push_str(&ellipsis);
            result.push(sep);
            result.push_str(last);
            return result;
        }

        let needed_for_ellipsis_last = ellipsis.len() + 1 + last.len(); // ellipsis + sep + last
        if needed_for_ellipsis_last <= max_size {
            let mut result = String::with_capacity(needed_for_ellipsis_last);
            result.push_str(&ellipsis);
            result.push(sep);
            result.push_str(last);
            return result;
        }

        // Can't fit ellipsis + last, just show as much of last as possible
        Self::truncate_str(last, max_size)
    }

    fn expand_middle(
        &self,
        components: &[&str],
        max_size: usize,
        use_number: bool,
        sep: char,
    ) -> String {
        let total = components.len();

        // Start with minimum: first/...or..N../last
        let mut left_end = 1; // exclusive index for left components
        let mut right_start = total - 1; // inclusive index for right components

        // Try to add more components from both sides
        loop {
            if right_start <= left_end {
                break;
            }

            let mut added = false;

            // Try adding from RIGHT first (to show more context near the file)
            if right_start > left_end + 1 {
                let hidden = right_start - 1 - left_end;
                let candidate = Self::build_middle_result(
                    components,
                    left_end,
                    right_start - 1,
                    hidden,
                    use_number,
                    sep,
                );
                if candidate.len() <= max_size {
                    right_start -= 1;
                    added = true;
                }
            }

            // Try adding from LEFT
            if left_end < right_start - 1 {
                let hidden = right_start - (left_end + 1);
                let candidate = Self::build_middle_result(
                    components,
                    left_end + 1,
                    right_start,
                    hidden,
                    use_number,
                    sep,
                );
                if candidate.len() <= max_size {
                    left_end += 1;
                    added = true;
                }
            }

            if !added {
                break;
            }
        }

        let hidden = right_start - left_end;
        Self::build_middle_result(components, left_end, right_start, hidden, use_number, sep)
    }

    fn build_middle_result(
        components: &[&str],
        left_end: usize,
        right_start: usize,
        hidden_count: usize,
        use_number: bool,
        sep: char,
    ) -> String {
        let ellipsis = Self::make_ellipsis(hidden_count, use_number);

        let left_parts = &components[..left_end];
        let right_parts = &components[right_start..];

        // Pre-calculate capacity
        let left_len: usize = left_parts.iter().map(|s| s.len()).sum();
        let right_len: usize = right_parts.iter().map(|s| s.len()).sum();
        let left_seps = if left_parts.is_empty() {
            0
        } else {
            left_parts.len() - 1
        };
        let right_seps = if right_parts.is_empty() {
            0
        } else {
            right_parts.len() - 1
        };
        // +2 for separators around ellipsis (or +1 if left is empty)
        let extra_seps = if left_parts.is_empty() { 1 } else { 2 };
        let capacity = left_len + right_len + left_seps + right_seps + ellipsis.len() + extra_seps;

        let mut result = String::with_capacity(capacity);

        // Build left part
        for (i, part) in left_parts.iter().enumerate() {
            if i > 0 {
                result.push(sep);
            }
            result.push_str(part);
        }

        // Add separator before ellipsis (only if left is not empty)
        if !left_parts.is_empty() {
            result.push(sep);
        }

        // Add ellipsis
        result.push_str(&ellipsis);

        // Add separator after ellipsis
        result.push(sep);

        // Build right part
        for (i, part) in right_parts.iter().enumerate() {
            if i > 0 {
                result.push(sep);
            }
            result.push_str(part);
        }

        result
    }

    fn make_ellipsis(hidden_count: usize, use_number: bool) -> Cow<'static, str> {
        match hidden_count {
            1 => ".".into(),
            2 => "..".into(),
            3 if use_number => "...".into(),
            n if use_number => format!(".{}.", n).into(),
            _ => "...".into(),
        }
    }
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

    #[test]
    fn test_path_shorten_strategy_middle() {
        // Test with directory paths (not file paths) - this is what Lua passes
        let path = Path::new("core_workflow_service/db/model/parts/ai_extracted");

        // With 25 chars, first component must be truncated
        // "core_workflow_service" is 21 chars, so we need to truncate it
        let shortened = PathShortenStrategy::Middle.shorten_path(path, 25);
        assert!(
            shortened.len() <= 25,
            "Result '{}' should be <= 25 chars",
            shortened
        );
        assert!(shortened.contains("..."), "Should contain ellipsis");
        assert!(
            shortened.ends_with("ai_extracted"),
            "Should end with last component"
        );

        // With 45 chars, can fit more without truncation
        let shortened = PathShortenStrategy::Middle.shorten_path(path, 45);
        assert!(shortened.len() <= 45);
        assert!(shortened.starts_with("core_workflow_service"));

        // Shorter path that fits better
        let path2 = Path::new("src/components/ui/buttons");
        let shortened = PathShortenStrategy::Middle.shorten_path(path2, 20);
        assert!(
            shortened.len() <= 20,
            "Result '{}' should be <= 20 chars",
            shortened
        );

        // Very small max_size - should still produce something reasonable
        let shortened = PathShortenStrategy::Middle.shorten_path(path2, 10);
        assert!(
            shortened.len() <= 10,
            "Result '{}' should be <= 10 chars",
            shortened
        );
    }

    #[test]
    fn test_path_shroten_strategy_middle_number() {
        // Test with directory paths (not file paths)
        // middle_number uses dots for 1-3 hidden, numbers for 4+

        // Path with only 2 hidden segments - should use dots
        let path = Path::new("core_workflow_service/graphql/types/parts");
        let shortened = PathShortenStrategy::MiddleNumber.shorten_path(path, 40);
        assert!(
            shortened.len() <= 40,
            "Result '{}' should be <= 40 chars",
            shortened
        );
        // With only 2 hidden, should use dots not numbers
        assert!(
            shortened.contains('.'),
            "Should contain dots, got '{}'",
            shortened
        );

        // Path with many segments, small space - should use .N. format when 4+ hidden
        let path2 = Path::new("a/b/c/d/e/f/g/h/i/j");
        let shortened = PathShortenStrategy::MiddleNumber.shorten_path(path2, 12);
        assert!(
            shortened.len() <= 12,
            "Result '{}' should be <= 12 chars",
            shortened
        );
        // With 8 hidden (showing only a and j), should show number
        assert!(
            shortened.contains('.') && shortened.chars().any(|c| c.is_ascii_digit()),
            "Should contain .N. pattern for 4+ hidden, got '{}'",
            shortened
        );

        // Very small max_size
        let shortened = PathShortenStrategy::MiddleNumber.shorten_path(path2, 5);
        assert!(
            shortened.len() <= 5,
            "Result '{}' should be <= 5 chars",
            shortened
        );
    }

    #[test]
    fn test_path_shroten_strategy_end() {
        let path = Path::new("core_workflow_service/db/model/parts/ai_extracted");
        let shortened = PathShortenStrategy::End.shorten_path(path, 25);
        assert!(shortened.len() <= 25);
        assert!(shortened.starts_with("core_workflow_service"));

        // Shorter constraint - truncates first component
        let shortened = PathShortenStrategy::End.shorten_path(path, 15);
        assert!(
            shortened.len() <= 15,
            "Result '{}' should be <= 15 chars",
            shortened
        );
    }

    #[test]
    fn test_shorten_path_caching() {
        let path = Path::new("home/user/projects/rust/project/src/components/ui");

        // First call should compute and cache
        let result1 = shorten_path(PathShortenStrategy::MiddleNumber, 25, path).unwrap();

        // Second call should hit cache
        let result2 = shorten_path(PathShortenStrategy::MiddleNumber, 25, path).unwrap();

        assert_eq!(result1, result2);

        // Different max_size should produce different result (more space = longer result)
        let result3 = shorten_path(PathShortenStrategy::MiddleNumber, 50, path).unwrap();
        assert!(
            result3.len() >= result1.len(),
            "More space should allow longer result"
        );
    }

    #[test]
    fn test_path_always_fits_max_size() {
        // Path must ALWAYS fit within max_size - this is a strict requirement
        let paths = [
            "core_workflow_service/db/model/parts/ai_extracted",
            "home/user/projects/rust/project/src",
            "a/b/c/d/e/f/g/h",
            "very_long_directory_name/another_long_one/and_more",
        ];

        for path_str in paths {
            let path = Path::new(path_str);
            for max_size in [10, 15, 20, 25, 30, 40, 50] {
                let shortened = PathShortenStrategy::MiddleNumber.shorten_path(path, max_size);
                assert!(
                    shortened.len() <= max_size,
                    "Path '{}' with max_size {} produced '{}' ({} chars)",
                    path_str,
                    max_size,
                    shortened,
                    shortened.len()
                );

                let shortened = PathShortenStrategy::Middle.shorten_path(path, max_size);
                assert!(
                    shortened.len() <= max_size,
                    "Path '{}' with max_size {} produced '{}' ({} chars)",
                    path_str,
                    max_size,
                    shortened,
                    shortened.len()
                );
            }
        }
    }

    #[test]
    fn test_small_max_size_simple_truncation() {
        // When max_size is very small (< MIN_SMART_SHORTEN_SIZE), should just truncate
        let path = Path::new("core_workflow_service/db/model/parts");

        // With max_size=6, should just truncate (below threshold)
        let shortened = PathShortenStrategy::MiddleNumber.shorten_path(path, 6);
        assert_eq!(shortened.len(), 6);
        assert_eq!(shortened, "core_w");

        // With max_size=10, smart shortening kicks in
        let shortened = PathShortenStrategy::Middle.shorten_path(path, 10);
        assert!(shortened.len() <= 10);
    }

    #[test]
    fn test_prioritizes_last_component() {
        // When space allows, last component should be shown in full
        let path = Path::new("first/medium/last_component");

        // With enough space, last component should be intact
        let shortened = PathShortenStrategy::MiddleNumber.shorten_path(path, 25);
        assert!(
            shortened.ends_with("last_component"),
            "Should preserve last component when space allows, got '{}'",
            shortened
        );
        assert!(shortened.len() <= 25);

        // When space is too tight, last component may be truncated to fit
        let shortened = PathShortenStrategy::MiddleNumber.shorten_path(path, 10);
        assert!(
            shortened.len() <= 10,
            "Must fit within max_size, got '{}' ({} chars)",
            shortened,
            shortened.len()
        );
    }
}
