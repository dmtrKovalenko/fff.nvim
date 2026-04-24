//! Constraint-based prefiltering for search queries.

use ahash::AHashSet;
use fff_query_parser::{Constraint, GitStatusFilter};
use smallvec::SmallVec;

use crate::git::is_modified_status;
use crate::simd_path::ArenaPtr;

/// `needle` must already be lowercase.
#[inline]
fn contains_ascii_ci(haystack: &str, needle: &str) -> bool {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.len() > h.len() {
        return false;
    }
    if n.is_empty() {
        return true;
    }
    let first = n[0];
    for i in 0..=(h.len() - n.len()) {
        if h[i].to_ascii_lowercase() == first
            && h[i..i + n.len()]
                .iter()
                .zip(n)
                .all(|(a, b)| a.to_ascii_lowercase() == *b)
        {
            return true;
        }
    }
    false
}

const PAR_THRESHOLD: usize = 10_000;

pub(crate) trait Constrainable {
    fn write_file_name(&self, arena: ArenaPtr, out: &mut String);
    fn git_status(&self) -> Option<git2::Status>;
    fn write_relative_path(&self, arena: ArenaPtr, out: &mut String);
}

/// Check if a relative path ends with the given suffix at a `/` boundary (case-insensitive).
///
/// Returns `true` when the path equals the suffix or the character before the suffix
/// in the path is `/`. This ensures partial directory-name matches are rejected.
///
/// Examples:
/// - `path_ends_with_suffix("libswscale/input.c", "libswscale/input.c")` → true (exact)
/// - `path_ends_with_suffix("foo/libswscale/input.c", "libswscale/input.c")` → true (suffix)
/// - `path_ends_with_suffix("xlibswscale/input.c", "libswscale/input.c")` → false (no boundary)
#[inline]
pub fn path_ends_with_suffix(path: &str, suffix: &str) -> bool {
    let path_bytes = path.as_bytes();
    let suffix_bytes = suffix.as_bytes();
    if path_bytes.len() < suffix_bytes.len() {
        return false;
    }

    let start = path.len() - suffix.len();

    // Must land on a char boundary — multi-byte UTF-8 can make
    // `path.len() - suffix.len()` land inside a char.
    if !path.is_char_boundary(start) {
        return false;
    }

    if !path[start..].eq_ignore_ascii_case(suffix) {
        return false;
    }

    // Exact match, or the character before is '/'.
    // `start` is a char boundary but `start - 1` may be inside a multi-byte
    // char, so scan backward to find the preceding ASCII byte.
    if start == 0 {
        return true;
    }
    let mut i = start;
    while i > 0 {
        i -= 1;
        // ASCII bytes (0..128) are single-byte UTF-8 code units
        if path_bytes[i] < 128 {
            return path_bytes[i] == b'/';
        }
    }
    false
}

#[inline]
pub fn file_has_extension(file_name: &str, ext: &str) -> bool {
    let name_bytes = file_name.as_bytes();
    let ext_bytes = ext.as_bytes();
    if name_bytes.len() <= ext_bytes.len() + 1 {
        return false;
    }
    let start = name_bytes.len() - ext_bytes.len() - 1;
    // `.` is ASCII (single byte), so `start` must be a char boundary.
    // If it lands inside a multi-byte char the extension can't match.
    if start > 0 && !file_name.is_char_boundary(start) {
        return false;
    }
    name_bytes.get(start) == Some(&b'.') && name_bytes[start + 1..].eq_ignore_ascii_case(ext_bytes)
}

/// Supports multi-segment paths like "libswscale/aarch64" (consecutive components).
#[inline]
pub fn path_contains_segment(path: &str, segment: &str) -> bool {
    let path_bytes = path.as_bytes();
    let segment_bytes = segment.as_bytes();
    let segment_len = segment_bytes.len();

    // Check segment/ at start of path
    if path_bytes.len() > segment_len
        && path_bytes.get(segment_len) == Some(&b'/')
        && path.is_char_boundary(segment_len)
        && path_bytes[..segment_len].eq_ignore_ascii_case(segment_bytes)
    {
        return true;
    }

    // Check /segment/ anywhere using byte scanning
    if path_bytes.len() < segment_len + 2 {
        return false;
    }

    for i in 0..path_bytes.len().saturating_sub(segment_len + 1) {
        if path_bytes[i] == b'/' {
            let start = i + 1;
            let end = start + segment_len;
            if end < path_bytes.len()
                && path_bytes[end] == b'/'
                && path.is_char_boundary(start)
                && path.is_char_boundary(end)
                && path_bytes[start..end].eq_ignore_ascii_case(segment_bytes)
            {
                return true;
            }
        }
    }
    false
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn item_matches_constraint_at_index<T: Constrainable>(
    item: &T,
    item_index: usize,
    constraint: &Constraint<'_>,
    glob_results: &[(bool, AHashSet<usize>)],
    glob_idx: &mut usize,
    negate: bool,
    arena: ArenaPtr,
    fname_buf: &mut String,
    path_buf: &mut String,
) -> bool {
    let matches = match constraint {
        Constraint::Extension(ext) => {
            item.write_file_name(arena, fname_buf);
            file_has_extension(fname_buf, ext)
        }
        Constraint::Glob(_) => {
            let result = glob_results
                .get(*glob_idx)
                .map(|(is_neg, set)| {
                    let matched = set.contains(&item_index);

                    if *is_neg { !matched } else { matched }
                })
                .unwrap_or(true);
            *glob_idx += 1;
            return if negate { !result } else { result };
        }
        Constraint::PathSegment(segment) => {
            item.write_relative_path(arena, path_buf);
            path_contains_segment(path_buf, segment)
        }
        Constraint::FilePath(suffix) => {
            item.write_relative_path(arena, path_buf);
            path_ends_with_suffix(path_buf, suffix)
        }
        Constraint::GitStatus(status_filter) => match (item.git_status(), status_filter) {
            (Some(status), GitStatusFilter::Modified) => is_modified_status(status),
            (Some(status), GitStatusFilter::Untracked) => status.contains(git2::Status::WT_NEW),
            (Some(status), GitStatusFilter::Staged) => status.intersects(
                git2::Status::INDEX_NEW
                    | git2::Status::INDEX_MODIFIED
                    | git2::Status::INDEX_DELETED
                    | git2::Status::INDEX_RENAMED
                    | git2::Status::INDEX_TYPECHANGE,
            ),
            (Some(status), GitStatusFilter::Unmodified) => status.is_empty(),
            (None, GitStatusFilter::Unmodified) => true,
            (None, _) => false,
        },
        Constraint::Not(inner) => {
            return item_matches_constraint_at_index(
                item,
                item_index,
                inner,
                glob_results,
                glob_idx,
                !negate,
                arena,
                fname_buf,
                path_buf,
            );
        }

        // only works with negation
        Constraint::Text(text) => {
            item.write_relative_path(arena, path_buf);
            contains_ascii_ci(path_buf, text)
        }

        // Parts and Exclude are handled at a higher level
        Constraint::Parts(_) | Constraint::Exclude(_) | Constraint::FileType(_) => true,
    };

    if negate { !matches } else { matches }
}

/// Returns `None` if no constraints are present, `Some(filtered)` otherwise.
/// Extension constraints use OR logic; all others use AND.
pub(crate) fn apply_constraints<'a, T: Constrainable + Sync>(
    items: &'a [T],
    constraints: &[Constraint<'_>],
    arena: ArenaPtr,
) -> Option<Vec<&'a T>> {
    if constraints.is_empty() {
        return None;
    }

    // Separate extension constraints from other constraints — they use OR logic
    let mut extensions: SmallVec<[&str; 8]> = SmallVec::new();
    let mut other_constraints: SmallVec<[&Constraint<'_>; 8]> = SmallVec::new();

    for constraint in constraints {
        match constraint {
            Constraint::Extension(ext) => extensions.push(ext),
            _ => other_constraints.push(constraint),
        }
    }

    // Only collect paths if we have glob constraints (expensive)
    let has_globs = other_constraints
        .iter()
        .any(|c| matches!(c, Constraint::Glob(_) | Constraint::Not(_)));

    let glob_results = if has_globs {
        // Build a single contiguous buffer of all relative paths + offset table.
        // One allocation for the buffer, one for offsets — NOT one String per file.
        let mut path_buf = Vec::<u8>::new();
        let mut offsets = Vec::<(usize, usize)>::with_capacity(items.len());
        let mut tmp = String::with_capacity(64);
        for item in items.iter() {
            let start = path_buf.len();
            item.write_relative_path(arena, &mut tmp);
            path_buf.extend_from_slice(tmp.as_bytes());
            offsets.push((start, path_buf.len() - start));
        }
        let path_refs: Vec<&str> = offsets
            .iter()
            .map(|&(off, len)| unsafe { std::str::from_utf8_unchecked(&path_buf[off..off + len]) })
            .collect();
        precompute_glob_matches(&other_constraints, &path_refs)
    } else {
        Vec::new()
    };

    let filtered: Vec<&T> = if items.len() >= PAR_THRESHOLD {
        use rayon::prelude::*;
        items
            .par_iter()
            .enumerate()
            .map_init(
                || (String::with_capacity(64), String::with_capacity(64)),
                |(fname_buf, path_buf), (i, item)| {
                    if !extensions.is_empty() {
                        item.write_file_name(arena, fname_buf);
                        if !extensions
                            .iter()
                            .any(|ext| file_has_extension(fname_buf, ext))
                        {
                            return None;
                        }
                    }

                    let mut glob_idx = 0;
                    if other_constraints.iter().all(|constraint| {
                        item_matches_constraint_at_index(
                            item,
                            i,
                            constraint,
                            &glob_results,
                            &mut glob_idx,
                            false,
                            arena,
                            fname_buf,
                            path_buf,
                        )
                    }) {
                        Some(item)
                    } else {
                        None
                    }
                },
            )
            .flatten()
            .collect()
    } else {
        let mut fname_buf = String::with_capacity(64);
        let mut path_buf = String::with_capacity(64);

        items
            .iter()
            .enumerate()
            .filter(|&(i, item)| {
                if !extensions.is_empty() {
                    item.write_file_name(arena, &mut fname_buf);
                    if !extensions
                        .iter()
                        .any(|ext| file_has_extension(&fname_buf, ext))
                    {
                        return false;
                    }
                }

                let mut glob_idx = 0;
                other_constraints.iter().all(|constraint| {
                    item_matches_constraint_at_index(
                        item,
                        i,
                        constraint,
                        &glob_results,
                        &mut glob_idx,
                        false,
                        arena,
                        &mut fname_buf,
                        &mut path_buf,
                    )
                })
            })
            .map(|(_, item)| item)
            .collect()
    };

    Some(filtered)
}

fn precompute_glob_matches<'a>(
    constraints: &[&Constraint<'a>],
    paths: &[&str],
) -> Vec<(bool, AHashSet<usize>)> {
    let mut results = Vec::new();
    for constraint in constraints {
        collect_glob_indices(constraint, paths, &mut results, false);
    }
    results
}

fn collect_glob_indices<'a>(
    constraint: &Constraint<'a>,
    paths: &[&str],
    results: &mut Vec<(bool, AHashSet<usize>)>,
    _is_negated: bool,
) {
    match constraint {
        Constraint::Glob(pattern) => {
            let indices = match_glob_pattern(pattern, paths);
            // Negation is handled by the `negate` parameter in
            // `item_matches_constraint_at_index`, NOT here.  Storing
            // `is_negated=true` caused a double-negation bug when the
            // Glob arm also applied `negate`.
            results.push((false, indices));
        }
        Constraint::Not(inner) => {
            collect_glob_indices(inner, paths, results, true);
        }
        _ => {}
    }
}

/// Match a glob pattern against a list of paths, returning the set of matching indices.
///
/// When the `zlob` feature is enabled, delegates to `zlob::zlob_match_paths` (Zig-compiled
/// C library, fastest). Otherwise falls back to `globset::Glob` (pure Rust).
#[cfg(feature = "zlob")]
fn match_glob_pattern(pattern: &str, paths: &[&str]) -> AHashSet<usize> {
    let Ok(Some(matches)) = zlob::zlob_match_paths(pattern, paths, zlob::ZlobFlags::RECOMMENDED)
    else {
        return AHashSet::new();
    };

    let matched_set: AHashSet<usize> = matches.iter().map(|s| s.as_ptr() as usize).collect();

    if paths.len() >= PAR_THRESHOLD {
        use rayon::prelude::*;
        paths
            .par_iter()
            .enumerate()
            .filter(|(_, p)| matched_set.contains(&(p.as_ptr() as usize)))
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
            .into_iter()
            .collect()
    } else {
        paths
            .iter()
            .enumerate()
            .filter(|(_, p)| matched_set.contains(&(p.as_ptr() as usize)))
            .map(|(i, _)| i)
            .collect()
    }
}

#[cfg(not(feature = "zlob"))]
fn match_glob_pattern(pattern: &str, paths: &[&str]) -> AHashSet<usize> {
    let Ok(glob) = globset::Glob::new(pattern) else {
        return AHashSet::new();
    };
    let matcher = glob.compile_matcher();

    if paths.len() >= PAR_THRESHOLD {
        use rayon::prelude::*;
        paths
            .par_iter()
            .enumerate()
            .filter(|(_, p)| matcher.is_match(p))
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
            .into_iter()
            .collect()
    } else {
        paths
            .iter()
            .enumerate()
            .filter(|(_, p)| matcher.is_match(p))
            .map(|(i, _)| i)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct TestItem {
        relative_path: &'static str,
        file_name: &'static str,
    }

    impl Constrainable for TestItem {
        fn write_file_name(&self, _arena: ArenaPtr, out: &mut String) {
            out.clear();
            out.push_str(self.file_name);
        }

        fn write_relative_path(&self, _arena: ArenaPtr, out: &mut String) {
            out.clear();
            out.push_str(self.relative_path);
        }

        fn git_status(&self) -> Option<git2::Status> {
            None
        }
    }

    #[test]
    fn test_file_has_extension() {
        assert!(file_has_extension("file.rs", "rs"));
        assert!(file_has_extension("file.RS", "rs")); // case-insensitive
        assert!(file_has_extension("file.test.rs", "rs"));
        assert!(file_has_extension("a.rs", "rs"));

        assert!(!file_has_extension("file.tsx", "rs"));
        assert!(!file_has_extension("rs", "rs")); // too short
        assert!(!file_has_extension(".rs", "rs")); // just extension
        assert!(!file_has_extension("file.rsx", "rs")); // different extension
        assert!(!file_has_extension("filers", "rs")); // no dot
    }

    #[test]
    fn test_path_contains_segment() {
        // Segment at start
        assert!(path_contains_segment("src/lib.rs", "src"));
        assert!(path_contains_segment("SRC/lib.rs", "src")); // case-insensitive

        // Segment in middle
        assert!(path_contains_segment("app/src/lib.rs", "src"));
        assert!(path_contains_segment("app/SRC/lib.rs", "src"));

        // Multiple levels
        assert!(path_contains_segment("core/workflow/src/main.rs", "src"));
        assert!(path_contains_segment(
            "core/workflow/src/main.rs",
            "workflow"
        ));
        assert!(path_contains_segment("core/workflow/src/main.rs", "core"));

        // Should not match partial segments
        assert!(!path_contains_segment("source/lib.rs", "src"));
        assert!(!path_contains_segment("mysrc/lib.rs", "src"));

        // Should not match filename
        assert!(!path_contains_segment("lib/src", "src"));

        // Multi-segment constraints
        assert!(path_contains_segment(
            "libswscale/aarch64/input.S",
            "libswscale/aarch64"
        ));
        assert!(path_contains_segment(
            "foo/libswscale/aarch64/input.S",
            "libswscale/aarch64"
        ));
        assert!(path_contains_segment(
            "foo/LibSwscale/AArch64/input.S",
            "libswscale/aarch64"
        )); // case-insensitive
        assert!(!path_contains_segment(
            "xlibswscale/aarch64/input.S",
            "libswscale/aarch64"
        )); // partial match at start
        assert!(!path_contains_segment(
            "foo/libswscale/aarch64x/input.S",
            "libswscale/aarch64"
        )); // partial match at end
        assert!(path_contains_segment(
            "crates/fff-core/src/grep.rs",
            "fff-core/src"
        ));

        // Edge cases
        assert!(!path_contains_segment("", "src"));
        assert!(!path_contains_segment("src", "src")); // no trailing slash
    }

    #[test]
    fn test_path_ends_with_suffix() {
        // Exact match
        assert!(path_ends_with_suffix(
            "libswscale/input.c",
            "libswscale/input.c"
        ));

        // Suffix match at / boundary
        assert!(path_ends_with_suffix(
            "foo/libswscale/input.c",
            "libswscale/input.c"
        ));

        // Deep nesting
        assert!(path_ends_with_suffix(
            "a/b/c/libswscale/input.c",
            "libswscale/input.c"
        ));

        // No boundary — partial directory name
        assert!(!path_ends_with_suffix(
            "xlibswscale/input.c",
            "libswscale/input.c"
        ));

        // Case insensitive
        assert!(path_ends_with_suffix(
            "foo/LibSwscale/Input.C",
            "libswscale/input.c"
        ));

        // Single file name
        assert!(path_ends_with_suffix("input.c", "input.c"));
        assert!(!path_ends_with_suffix("xinput.c", "input.c"));

        // Suffix longer than path
        assert!(!path_ends_with_suffix("input.c", "foo/input.c"));

        // Simple path
        assert!(path_ends_with_suffix("src/main.rs", "src/main.rs"));
        assert!(path_ends_with_suffix("crates/src/main.rs", "src/main.rs"));
    }

    #[test]
    fn test_path_ends_with_suffix_does_not_panic_on_unicode_suffix() {
        assert!(!path_ends_with_suffix("유니코드_파일_테스트.csv", "트.c"));
        assert!(path_ends_with_suffix(
            "data/유니코드_파일_테스트.csv",
            "유니코드_파일_테스트.csv"
        ));
    }

    #[test]
    fn test_path_ends_with_suffix_unicode_apostrophe_mismatch() {
        assert!(!path_ends_with_suffix(
            "dir/\u{2019}bar/file.txt",
            "'bar/file.txt"
        ));
    }

    #[test]
    fn test_path_ends_with_suffix_unicode_space_mismatch() {
        assert!(!path_ends_with_suffix(
            "dir/\u{202f}am/file.txt",
            " am/file.txt"
        ));
    }

    #[test]
    fn test_path_contains_segment_does_not_panic_on_unicode_segment() {
        assert!(!path_contains_segment("문서/notes.txt", "문x"));
        assert!(path_contains_segment("프로젝트/문서/notes.txt", "문서"));
    }

    #[test]
    fn test_path_contains_segment_unicode_no_panic() {
        assert!(!path_contains_segment(
            "Library/Cloud/Project\u{2019}s Folder/books.ttl",
            "Project's Folder"
        ));
    }

    #[test]
    fn test_file_has_extension_unicode_no_panic() {
        assert!(!file_has_extension("cat\u{00e9}.rs", "s"));
    }

    #[test]
    fn test_file_has_extension_unicode_filename() {
        assert!(file_has_extension("운영-가이드.md", "md"));
        assert!(file_has_extension("테스트.csv", "csv"));
        assert!(!file_has_extension("테스트.csv", "md"));
    }

    #[test]
    fn test_apply_constraints_file_path_with_unicode_suffix() {
        let arena_ptr = ArenaPtr(std::ptr::null());

        let item = TestItem {
            relative_path: "data/유니코드_파일_테스트.csv",
            file_name: "유니코드_파일_테스트.csv",
        };

        let exact = [Constraint::FilePath("유니코드_파일_테스트.csv")];
        let mismatch = [Constraint::FilePath("트.c")];

        let exact_items = [item.clone()];
        let exact_matches =
            apply_constraints(&exact_items, &exact, arena_ptr).expect("constraints applied");
        assert_eq!(exact_matches.len(), 1);

        let mismatch_items = [item];
        let mismatch_matches =
            apply_constraints(&mismatch_items, &mismatch, arena_ptr).expect("constraints applied");
        assert!(mismatch_matches.is_empty());
    }

    #[test]
    fn test_unicode_path_no_panic_real_korean_cases() {
        // Real Korean paths that caused panics
        let path1 = "Downloads/(커리큘럼) hermes agent_정승현님 - 1차 커리큘럼 (강사님 작성).csv";
        let path2 = "hermes-agent-lecture-materials/세부_커리큘럼_최종.csv";
        let path3 = "projects/fastcampus-hermes-agent-curriculum/chapters/part-02-Hermes-설치-및-기본-사용/section-02-doctor로-설치-상태-검증/research/03-fix가-자동-수정하는-것과-못하는-것.md";

        // These must not panic regardless of segment/suffix used
        assert!(!path_contains_segment(path1, "작성"));
        assert!(!path_ends_with_suffix(path1, "작성.csv"));
        assert!(!path_contains_segment(path2, "최종"));
        assert!(!path_ends_with_suffix(path2, "최종.csv"));
        assert!(!path_contains_segment(path3, "수정"));
        assert!(!path_ends_with_suffix(path3, "것.md"));

        // Positive cases should still work
        assert!(path_contains_segment(
            path2,
            "hermes-agent-lecture-materials"
        ));
        assert!(path_ends_with_suffix(
            path1,
            "(커리큘럼) hermes agent_정승현님 - 1차 커리큘럼 (강사님 작성).csv"
        ));
        assert!(path_ends_with_suffix(path2, "세부_커리큘럼_최종.csv"));
    }

    #[test]
    fn test_negated_glob_excludes_matching_files() {
        let arena_ptr = ArenaPtr(std::ptr::null());

        let items = vec![
            TestItem {
                relative_path: "src/main.rs",
                file_name: "main.rs",
            },
            TestItem {
                relative_path: "src/lib.ts",
                file_name: "lib.ts",
            },
            TestItem {
                relative_path: "include/fff.h",
                file_name: "fff.h",
            },
        ];

        // Not(Glob("**/*.rs")) should exclude .rs files
        let constraints = vec![Constraint::Not(Box::new(Constraint::Glob("**/*.rs")))];
        let result = apply_constraints(&items, &constraints, arena_ptr).unwrap();
        let paths: Vec<&str> = result.iter().map(|i| i.relative_path).collect();
        assert!(
            !paths.contains(&"src/main.rs"),
            "rs file should be excluded"
        );
        assert!(paths.contains(&"src/lib.ts"), "ts file should be included");
        assert!(
            paths.contains(&"include/fff.h"),
            "h file should be included"
        );
    }
}
