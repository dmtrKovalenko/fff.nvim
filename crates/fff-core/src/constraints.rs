//! Constraint filtering engine for fff.
//!
//! This module provides the core constraint application logic that filters items
//! based on parsed query constraints (extensions, path segments, globs, git status, etc.).
//!
//! The filtering is generic over the [`Constrainable`] trait, allowing reuse across
//! different search modes (file picker, live grep, etc.).

use ahash::AHashSet;
use fff_query_parser::{Constraint, GitStatusFilter};
use smallvec::SmallVec;

use crate::git::is_modified_status;

/// Minimum item count before switching to parallel iteration with rayon.
/// Below this threshold, the overhead of thread pool dispatch outweighs the benefit.
const PAR_THRESHOLD: usize = 10_000;

/// Trait for items that can be filtered by constraints.
/// Implement this for any searchable item type (files, grep results, etc.).
pub trait Constrainable {
    /// The file's relative path (e.g. "src/main.rs")
    fn relative_path(&self) -> &str;

    /// The file's lowercased relative path for case-insensitive matching
    fn relative_path_lower(&self) -> &str;

    /// The file name component (e.g. "main.rs")
    fn file_name(&self) -> &str;

    /// The git status of this item, if available
    fn git_status(&self) -> Option<git2::Status>;
}

/// Check if file extension matches (without allocation)
#[inline]
pub fn file_has_extension(file_name: &str, ext: &str) -> bool {
    if file_name.len() <= ext.len() + 1 {
        return false;
    }
    let start = file_name.len() - ext.len() - 1;
    file_name.as_bytes().get(start) == Some(&b'.')
        && file_name[start + 1..].eq_ignore_ascii_case(ext)
}

/// Check if path contains segment (without allocation)
#[inline]
pub fn path_contains_segment(path: &str, segment: &str) -> bool {
    let path_bytes = path.as_bytes();
    let segment_len = segment.len();

    // Check segment/ at start
    if path.len() > segment_len
        && path_bytes.get(segment_len) == Some(&b'/')
        && path[..segment_len].eq_ignore_ascii_case(segment)
    {
        return true;
    }

    // Check /segment/ anywhere using byte scanning
    if path.len() < segment_len + 2 {
        return false;
    }

    for i in 0..path.len().saturating_sub(segment_len + 1) {
        if path_bytes[i] == b'/' {
            let start = i + 1;
            let end = start + segment_len;
            if end < path.len()
                && path_bytes[end] == b'/'
                && path[start..end].eq_ignore_ascii_case(segment)
            {
                return true;
            }
        }
    }
    false
}

/// Check if an item at given index matches a constraint (single-pass friendly, allocation-free)
#[inline]
fn item_matches_constraint_at_index<T: Constrainable>(
    item: &T,
    item_index: usize,
    constraint: &Constraint<'_>,
    glob_results: &[(bool, AHashSet<usize>)],
    glob_idx: &mut usize,
    negate: bool,
) -> bool {
    let matches = match constraint {
        Constraint::Extension(ext) => file_has_extension(item.file_name(), ext),
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
        Constraint::PathSegment(segment) => path_contains_segment(item.relative_path(), segment),
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
            );
        }

        // only works with negation
        Constraint::Text(text) => item.relative_path_lower().contains(text),

        // Parts and Exclude are handled at a higher level
        Constraint::Parts(_) | Constraint::Exclude(_) | Constraint::FileType(_) => true,
    };

    if negate { !matches } else { matches }
}

/// Apply constraint-based prefiltering in a single pass over all items.
/// Returns `None` if no constraints are present, `Some(filtered)` otherwise.
/// Multiple extension constraints (*.rs *.ts) are combined with OR logic.
/// All other constraints are combined with AND logic.
///
/// Uses parallel iteration via rayon when the item count exceeds [`PAR_THRESHOLD`].
pub fn apply_constraints<'a, T: Constrainable + Sync>(
    items: &'a [T],
    constraints: &[Constraint<'_>],
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
        let paths: Vec<&str> = items.iter().map(|f| f.relative_path()).collect();
        precompute_glob_matches(&other_constraints, &paths)
    } else {
        Vec::new()
    };

    let matches_constraints = |i: usize, item: &T| -> bool {
        if !extensions.is_empty()
            && !extensions
                .iter()
                .any(|ext| file_has_extension(item.file_name(), ext))
        {
            return false;
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
            )
        })
    };

    let filtered: Vec<&T> = if items.len() >= PAR_THRESHOLD {
        use rayon::prelude::*;
        items
            .par_iter()
            .enumerate()
            .filter(|(i, item)| matches_constraints(*i, item))
            .map(|(_, item)| item)
            .collect()
    } else {
        items
            .iter()
            .enumerate()
            .filter(|(i, item)| matches_constraints(*i, item))
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
    is_negated: bool,
) {
    match constraint {
        Constraint::Glob(pattern) => {
            let indices = match_glob_pattern(pattern, paths);
            results.push((is_negated, indices));
        }
        Constraint::Not(inner) => {
            collect_glob_indices(inner, paths, results, !is_negated);
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

        // Edge cases
        assert!(!path_contains_segment("", "src"));
        assert!(!path_contains_segment("src", "src")); // no trailing slash
    }
}
