use crate::{
    constraints::apply_constraints,
    git::is_modified_status,
    path_utils::calculate_distance_penalty,
    sort_buffer::{sort_by_key_with_buffer, sort_with_buffer},
    types::{FileItem, Score, ScoringContext},
};
use fff_query_parser::FuzzyQuery;
use neo_frizbee::Scoring;
use rayon::prelude::*;
use std::path::MAIN_SEPARATOR;

// like cow but better
pub(crate) enum FileItems<'a> {
    /// All files — borrows the original owned slice, zero allocation.
    All(&'a [FileItem]),
    /// Filtered subset — owns references produced by constraint filtering.
    Filtered(Vec<&'a FileItem>),
}

impl<'a> FileItems<'a> {
    #[inline]
    #[allow(dead_code)]
    fn len(&self) -> usize {
        match self {
            FileItems::All(s) => s.len(),
            FileItems::Filtered(v) => v.len(),
        }
    }

    #[inline]
    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Index into the file list. Panics if out of bounds (like slice indexing).
    #[inline]
    fn index(&self, index: usize) -> &'a FileItem {
        match self {
            FileItems::All(s) => &s[index],
            FileItems::Filtered(v) => v[index],
        }
    }
}

/// Match files against all fuzzy parts.
/// Single part: use optimized batch matching.
/// Multiple parts: each part must match, scores are summed (Nucleo-style).
/// Parts with less than 2 characters are skipped.
///
/// Files are passed directly to frizbee via the `Matchable` trait —
/// deleted files return `None` from `match_str()` and are skipped
/// without any intermediate allocation.
#[inline]
fn match_fuzzy_parts(
    fuzzy_parts: &[&str],
    working_files: &FileItems<'_>,
    options: &neo_frizbee::Config,
    max_threads: usize,
) -> Vec<neo_frizbee::Match> {
    // Filter out parts that are too short (< 2 chars)
    let valid_parts: Vec<&str> = fuzzy_parts
        .iter()
        .copied()
        .filter(|p| p.len() >= 2)
        .collect();

    if valid_parts.is_empty() {
        tracing::debug!("match_fuzzy_parts: no valid parts after filtering, returning empty");
        return vec![];
    }

    let first_part_matches = match working_files {
        FileItems::All(files) => {
            neo_frizbee::match_list_parallel(valid_parts[0], files, options, max_threads)
        }
        FileItems::Filtered(files) => {
            neo_frizbee::match_list_parallel(valid_parts[0], files, options, max_threads)
        }
    };

    if valid_parts.len() == 1 {
        return first_part_matches;
    }

    // Multiple parts - match first part, then filter by remaining parts
    let mut matches = first_part_matches;
    for part in valid_parts[1..].iter() {
        let mut part_options = *options;
        part_options.max_typos = options.max_typos.map(|t| t.min(part.len() as u16));

        matches = matches
            .into_iter()
            .filter_map(|mut m| {
                let file = working_files.index(m.index as usize);
                let path = file.relative_path();
                let part_matches = neo_frizbee::match_list(part, &[path], &part_options);
                let part_match = part_matches.first()?;

                // Sum scores
                let total = (m.score as u32).saturating_add(part_match.score as u32);
                m.score = total.min(u16::MAX as u32) as u16;
                Some(m)
            })
            .collect();

        if matches.is_empty() {
            break;
        }
    }

    matches
}

pub fn match_and_score_files<'a>(
    files: &'a [FileItem],
    context: &ScoringContext,
) -> (Vec<&'a FileItem>, Vec<Score>, usize) {
    if files.is_empty() {
        return (vec![], vec![], 0);
    }

    let parsed = context.query;
    let working_files: FileItems<'a> = if parsed.constraints.is_empty() {
        FileItems::All(files)
    } else {
        match apply_constraints(files, &parsed.constraints) {
            Some(filtered) if !filtered.is_empty() => FileItems::Filtered(filtered),
            Some(_) => {
                return (vec![], vec![], 0);
            }
            None => FileItems::All(files),
        }
    };

    let fuzzy_parts: &[&str] = match &parsed.fuzzy_query {
        FuzzyQuery::Text(t) if t.len() >= 2 => std::slice::from_ref(t),
        FuzzyQuery::Parts(parts) if !parts.is_empty() => parts.as_slice(),
        _ => {
            return score_filtered_by_frecency(&working_files, context);
        }
    };
    debug_assert!(!fuzzy_parts.is_empty());

    let has_uppercase = fuzzy_parts
        .iter()
        .any(|p| p.chars().any(|c| c.is_uppercase()));
    let query_contains_path_separator = fuzzy_parts.iter().any(|p| p.contains(MAIN_SEPARATOR));

    let options = neo_frizbee::Config {
        max_typos: Some(context.max_typos),
        sort: false,
        scoring: Scoring {
            capitalization_bonus: if has_uppercase { 8 } else { 0 },
            matching_case_bonus: if has_uppercase { 4 } else { 0 },
            ..Default::default()
        },
    };

    let path_matches =
        match_fuzzy_parts(fuzzy_parts, &working_files, &options, context.max_threads);

    let main_needle = fuzzy_parts[0].as_bytes(); // safe
    let main_needle_len = main_needle.len() as u16;

    // Filename match detection: two tiers, cursor-based (no intermediate bitset/Vec<bool>).
    // 1) Collect filenames only where match_end_col didn't land in the filename region.
    // 2) Batch SIMD on that subset, remap indices, sort for cursor walk in the scoring loop.
    let mut fallback_indices: Vec<u32> = Vec::new();
    let filename_fallback_matches = if query_contains_path_separator || path_matches.len() > 15_000
    {
        vec![]
    } else {
        let mut fallback_filenames: Vec<&str> = Vec::new();

        for (i, path_match) in path_matches.iter().enumerate() {
            let file = working_files.index(path_match.index as usize);
            let filename_start = file.filename_offset_in_relative() as u16;
            let match_start_approx = path_match.end_col.saturating_sub(main_needle_len - 1);

            if match_start_approx < filename_start {
                fallback_indices.push(i as u32);
                fallback_filenames.push(file.file_name());
            }
        }

        if fallback_filenames.is_empty() {
            vec![]
        } else {
            let mut matches = neo_frizbee::match_list_parallel(
                fuzzy_parts[0],
                &fallback_filenames,
                &options,
                if path_matches.len() > 10_000 {
                    context.max_threads
                } else {
                    1
                },
            );

            sort_by_key_with_buffer(&mut matches, |m| fallback_indices[m.index as usize]);
            matches
        }
    };

    let mut next_filename_match_cursor = 0;
    let results: Vec<_> = path_matches
        .into_iter()
        .enumerate()
        .map(|(match_idx, path_match)| {
            let file_idx = path_match.index as usize;
            let file = working_files.index(file_idx);

            let base_score = path_match.score as i32;
            let frecency_boost = base_score.saturating_mul(file.total_frecency_score()) / 100;

            // Give modified/dirty files a 15% boost to make them appear higher in results
            let git_status_boost = if file.git_status.is_some_and(is_modified_status) {
                base_score * 15 / 100
            } else {
                0
            };

            let distance_penalty =
                calculate_distance_penalty(context.current_file, file.relative_path());

            let filename_start = file.filename_offset_in_relative() as u16;
            let match_start_approx = path_match.end_col.saturating_sub(main_needle_len - 1);

            let end_col_filename_match = match_start_approx >= filename_start;
            let simd_filename_match = if !end_col_filename_match {
                filename_fallback_matches
                    .get(next_filename_match_cursor)
                    .and_then(|m| {
                        if fallback_indices[m.index as usize] == match_idx as u32 {
                            next_filename_match_cursor += 1;
                            Some(m)
                        } else {
                            None
                        }
                    })
            } else {
                None
            };

            let is_filename_match = end_col_filename_match || simd_filename_match.is_some();
            let is_exact_filename = simd_filename_match.is_some_and(|m| m.exact)
                || (end_col_filename_match
                    && main_needle_len as usize == file.file_name().len()
                    && main_needle.eq_ignore_ascii_case(file.file_name().as_bytes()));

            let mut has_special_filename_bonus = false;
            let filename_bonus = if is_exact_filename {
                base_score / 5 * 2 // 40% bonus for exact filename match
            } else if is_filename_match {
                // 16% bonus for fuzzy filename match that landed in the filename region.
                // For fallback matches (where the path match landed in a directory segment),
                // scale the bonus by the quality of the filename match — a contiguous match
                // like "rename" in "rename.ts" gets the full bonus, while a scattered
                // subsequence like r-e-n-a-m-e in "generateSessionName.ts" gets much less.
                let max_bonus = (base_score / 6).min(30);
                if let Some(fm) = simd_filename_match {
                    let max_possible = main_needle_len as i32 * 16;
                    let quality = (fm.score as i32).min(max_possible);
                    max_bonus * quality / max_possible
                } else {
                    max_bonus
                }
            } else if !is_filename_match && is_special_entry_point_file(file.file_name()) {
                // 5% bonus for special file but not as much as file name to avoid situations
                // when you have /user_service/server.rs and /user_service/server/mod.rs
                has_special_filename_bonus = true;
                base_score * 5 / 100
            } else {
                0
            };

            let current_file_penalty = calculate_current_file_penalty(file, base_score, context);
            let combo_match_boost = {
                let last_same_query_match = context
                    .last_same_query_match
                    .as_ref()
                    .filter(|m| m.file_path.as_os_str() == file.as_path().as_os_str());

                match last_same_query_match {
                    // if we request a combo match without a boost we have to render it anyway
                    Some(_) if context.min_combo_count == 0 => 1000,
                    Some(combo_match) if combo_match.open_count >= context.min_combo_count => {
                        combo_match.open_count as i32 * context.combo_boost_score_multiplier
                    }
                    // until we hit the combo count threshold, we add a smaller boost because it
                    // makes sense and makes the search more efficient
                    Some(combo_match) => combo_match.open_count as i32 * 5,
                    _ => 0,
                }
            };

            let total = base_score
                .saturating_add(frecency_boost)
                .saturating_add(git_status_boost)
                .saturating_add(distance_penalty)
                .saturating_add(filename_bonus)
                .saturating_add(current_file_penalty)
                .saturating_add(combo_match_boost);

            let score = Score {
                total,
                base_score,
                current_file_penalty,
                filename_bonus,
                special_filename_bonus: if has_special_filename_bonus {
                    filename_bonus
                } else {
                    0
                },
                frecency_boost,
                git_status_boost,
                distance_penalty,
                combo_match_boost,
                exact_match: is_exact_filename || path_match.exact,
                match_type: if is_exact_filename {
                    "exact_filename"
                } else if is_filename_match {
                    "fuzzy_filename"
                } else if path_match.exact {
                    "exact_path"
                } else {
                    "fuzzy_path"
                },
            };

            (file, score)
        })
        .collect();

    sort_and_paginate(results, context)
}

/// Check if a filename is a special entry point file that deserves bonus scoring
/// These are typically files that serve as module exports or entry points
fn is_special_entry_point_file(filename: &str) -> bool {
    matches!(
        filename,
        "mod.rs"
            | "lib.rs"
            | "main.rs"
            | "index.js"
            | "index.jsx"
            | "index.ts"
            | "index.tsx"
            | "index.mjs"
            | "index.cjs"
            | "index.vue"
            | "__init__.py"
            | "__main__.py"
            | "main.go"
            | "main.c"
            | "index.php"
            | "main.rb"
            | "index.rb"
    )
}

/// Score files by frecency when we have a filtered list (prefiltered by constraints)
pub(crate) fn score_filtered_by_frecency<'a>(
    files: &FileItems<'a>,
    context: &ScoringContext,
) -> (Vec<&'a FileItem>, Vec<Score>, usize) {
    let score_file = |file: &'a FileItem| {
        let total_frecency_score = file.access_frecency_score as i32
            + (file.modification_frecency_score as i32).saturating_mul(4);

        // Give modified/dirty files a boost even in frecency-only mode
        let git_status_boost = if file.git_status.is_some_and(is_modified_status) {
            total_frecency_score * 15 / 100
        } else {
            0
        };

        let current_file_penalty =
            calculate_current_file_penalty(file, total_frecency_score, context);
        let total = total_frecency_score
            .saturating_add(git_status_boost)
            .saturating_add(current_file_penalty);

        let score = Score {
            total,
            base_score: 0,
            filename_bonus: 0,
            distance_penalty: 0,
            special_filename_bonus: 0,
            combo_match_boost: 0,
            current_file_penalty,
            frecency_boost: total_frecency_score,
            git_status_boost,
            exact_match: false,
            match_type: "frecency",
        };

        (file, score)
    };

    let results: Vec<_> = match files {
        FileItems::All(s) => s
            .par_iter()
            .filter(|f| !f.is_deleted())
            .map(&score_file)
            .collect(),
        FileItems::Filtered(v) => v
            .iter()
            .filter(|f| !f.is_deleted())
            .map(|&file| score_file(file))
            .collect(),
    };

    sort_and_paginate(results, context)
}

#[inline]
fn calculate_current_file_penalty(
    file: &FileItem,
    base_score: i32,
    context: &ScoringContext,
) -> i32 {
    let mut penalty = 0i32;

    if let Some(current) = context.current_file
        && file.relative_path() == current
    {
        penalty -= match file.git_status {
            Some(status) if is_modified_status(status) => base_score / 2,
            _ => base_score,
        };
    }

    penalty
}

/// Sorts elements by total score (descending) and returns the requested page.
/// Always returns results in descending order (best scores first).
/// The UI layer handles rendering order based on prompt position.
#[tracing::instrument(skip_all, level = tracing::Level::DEBUG)]
fn sort_and_paginate<'a>(
    mut results: Vec<(&'a FileItem, Score)>,
    context: &ScoringContext,
) -> (Vec<&'a FileItem>, Vec<Score>, usize) {
    let total_matched = results.len();

    if total_matched == 0 {
        return (vec![], vec![], 0);
    }

    let offset = context.pagination.offset;
    let limit = if context.pagination.limit > 0 {
        context.pagination.limit
    } else {
        total_matched
    };

    // Check if offset is out of bounds
    if offset >= total_matched {
        tracing::warn!(
            offset = offset,
            total_matched = total_matched,
            "Pagination: offset >= total_matched, returning empty"
        );

        return (vec![], vec![], total_matched);
    }

    let items_needed = offset.saturating_add(limit).min(total_matched);
    // Use partial sort if we need less than half the results and dataset is large
    let use_partial_sort = items_needed < total_matched / 2 && total_matched > 100;
    // Always sort in descending order (best scores first)
    if use_partial_sort {
        // Partition at position (items_needed - 1) with descending comparator
        // This puts the highest N needed items at the front
        results.select_nth_unstable_by(items_needed - 1, |a, b| {
            b.1.total
                .cmp(&a.1.total)
                .then_with(|| b.0.modified.cmp(&a.0.modified))
        });
        results.truncate(items_needed);
    }

    // select nth does not sort the results, we have to sort accordingly anyway
    sort_with_buffer(&mut results, |a, b| {
        b.1.total
            .cmp(&a.1.total)
            .then_with(|| b.0.modified.cmp(&a.0.modified))
    });

    // in the best scenario truncation happened in the select_nth step
    if results.len() > limit {
        let page_end = std::cmp::min(offset + limit, results.len());
        let page_size = page_end - offset;

        results.drain(0..offset);
        results.truncate(page_size);
    }

    let (items, scores): (Vec<&FileItem>, Vec<Score>) = results.into_iter().unzip();
    (items, scores, total_matched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PaginationArgs;
    use fff_query_parser::QueryParser;

    fn create_test_file(path: &str, score: i32, modified: u64) -> (FileItem, Score) {
        let filename_start = path.rfind('/').map(|i| i + 1).unwrap_or(0) as u16;
        let file = FileItem::new_raw(
            path.to_string(),
            0,
            filename_start,
            0,
            modified,
            None,
            false,
        );
        let score_obj = Score {
            total: score,
            base_score: score,
            filename_bonus: 0,
            distance_penalty: 0,
            special_filename_bonus: 0,
            current_file_penalty: 0,
            frecency_boost: 0,
            git_status_boost: 0,
            exact_match: false,
            match_type: "test",
            combo_match_boost: 0,
        };
        (file, score_obj)
    }

    #[test]
    fn test_partial_sort_descending() {
        // Create test data with known scores
        let test_data = vec![
            create_test_file("file1.rs", 100, 1000),
            create_test_file("file2.rs", 200, 2000),
            create_test_file("file3.rs", 50, 3000),
            create_test_file("file4.rs", 300, 4000),
            create_test_file("file5.rs", 150, 5000),
            create_test_file("file6.rs", 250, 6000),
            create_test_file("file7.rs", 80, 7000),
            create_test_file("file8.rs", 180, 8000),
            create_test_file("file9.rs", 120, 9000),
            create_test_file("file10.rs", 90, 10000),
        ];

        // Convert to references like the actual function uses
        let results: Vec<(&FileItem, Score)> = test_data
            .iter()
            .map(|(file, score)| (file, score.clone()))
            .collect();

        let query_str = "test";
        let parser = QueryParser::default();
        let query = parser.parse(query_str);
        let context = ScoringContext {
            query: &query,
            max_threads: 1,
            max_typos: 2,
            current_file: None,
            last_same_query_match: None,
            project_path: None,
            combo_boost_score_multiplier: 100,
            min_combo_count: 3,

            pagination: PaginationArgs {
                offset: 0,
                limit: 0,
            },
        };

        // Test with full sort - returns all results sorted descending
        let (items, scores, total) = sort_and_paginate(results.clone(), &context);

        // Should return all 10 items sorted by score descending
        assert_eq!(total, 10);
        assert_eq!(scores.len(), 10);
        assert_eq!(scores[0].total, 300, "First should be highest score");
        assert_eq!(scores[1].total, 250, "Second should be second highest");
        assert_eq!(scores[2].total, 200, "Third should be third highest");

        // Verify the files match
        assert_eq!(items[0].relative_path(), "file4.rs");
        assert_eq!(items[1].relative_path(), "file6.rs");
        assert_eq!(items[2].relative_path(), "file2.rs");
    }

    #[test]
    fn test_partial_sort_with_same_scores() {
        // Test tiebreaker with modified time
        let test_data = [
            create_test_file("file1.rs", 100, 5000), // Same score, older
            create_test_file("file2.rs", 100, 8000), // Same score, newer
            create_test_file("file3.rs", 100, 3000), // Same score, oldest
            create_test_file("file4.rs", 200, 1000),
            create_test_file("file5.rs", 200, 9000), // Higher score, newest
        ];

        let results: Vec<(&FileItem, Score)> = test_data
            .iter()
            .map(|(file, score)| (file, score.clone()))
            .collect();

        let query_str = "test";
        let parser = QueryParser::default();
        let query = parser.parse(query_str);
        let context = ScoringContext {
            query: &query,
            max_threads: 1,
            max_typos: 2,
            current_file: None,
            last_same_query_match: None,
            project_path: None,
            combo_boost_score_multiplier: 100,
            min_combo_count: 3,

            pagination: PaginationArgs {
                offset: 0,
                limit: 0,
            },
        };

        let (items, scores, _) = sort_and_paginate(results, &context);

        // Should return all 5 items sorted: 200(9000), 200(1000), 100(8000), 100(5000), 100(3000)
        assert_eq!(scores.len(), 5);
        assert_eq!(scores[0].total, 200);
        assert_eq!(items[0].modified, 9000, "First 200 should be newest");
        assert_eq!(scores[1].total, 200);
        assert_eq!(items[1].modified, 1000, "Second 200 should be older");
        assert_eq!(scores[2].total, 100);
        assert_eq!(items[2].modified, 8000, "First 100 should be newest");
        assert_eq!(scores[3].total, 100);
        assert_eq!(items[3].modified, 5000);
        assert_eq!(scores[4].total, 100);
        assert_eq!(items[4].modified, 3000, "Last 100 should be oldest");
    }

    #[test]
    fn test_no_partial_sort_for_small_results() {
        // When results.len() <= threshold, should use regular sort
        let test_data = [
            create_test_file("file1.rs", 100, 1000),
            create_test_file("file2.rs", 200, 2000),
            create_test_file("file3.rs", 50, 3000),
        ];

        let results: Vec<(&FileItem, Score)> = test_data
            .iter()
            .map(|(file, score)| (file, score.clone()))
            .collect();

        let query_str = "test";
        let parser = QueryParser::default();
        let query = parser.parse(query_str);
        let context = ScoringContext {
            query: &query,
            max_threads: 1,
            max_typos: 2,
            current_file: None,
            last_same_query_match: None,
            project_path: None,
            combo_boost_score_multiplier: 100,
            min_combo_count: 3,

            pagination: PaginationArgs {
                offset: 0,
                limit: 0,
            },
        };

        // Returns all results sorted descending
        let (items, scores, _) = sort_and_paginate(results, &context);

        assert_eq!(scores.len(), 3);
        assert_eq!(scores[0].total, 200);
        assert_eq!(scores[1].total, 100);
        assert_eq!(scores[2].total, 50);
        assert_eq!(items[0].relative_path(), "file2.rs");
        assert_eq!(items[1].relative_path(), "file1.rs");
        assert_eq!(items[2].relative_path(), "file3.rs");
    }
}

#[cfg(test)]
mod filename_bonus_tests {
    use super::*;
    use crate::types::PaginationArgs;
    use fff_query_parser::QueryParser;

    fn make_file(path: &str) -> FileItem {
        let filename_start = path.rfind('/').map(|i| i + 1).unwrap_or(0) as u16;
        FileItem::new_raw(path.to_string(), 0, filename_start, 0, 0, None, false)
    }

    fn search(files: &[FileItem], query: &str) -> Vec<(String, Score)> {
        let parser = QueryParser::default();
        let parsed = parser.parse(query);
        let ctx = ScoringContext {
            query: &parsed,
            max_threads: 1,
            max_typos: 2,
            current_file: None,
            last_same_query_match: None,
            project_path: None,
            combo_boost_score_multiplier: 100,
            min_combo_count: 3,
            pagination: PaginationArgs {
                offset: 0,
                limit: 100,
            },
        };
        let (items, scores, _) = match_and_score_files(files, &ctx);
        items
            .iter()
            .zip(scores.iter())
            .map(|(f, s)| (f.relative_path().to_string(), s.clone()))
            .collect()
    }

    #[test]
    fn test_filename_match_ranks_above_path_only_match() {
        let files = vec![
            make_file("src/username/handler.rs"),
            make_file("src/username/username.rs"),
        ];

        let results = search(&files, "usrnmea");

        assert!(
            results.len() >= 2,
            "both files should match, got {}",
            results.len()
        );
        assert_eq!(
            results[0].0, "src/username/username.rs",
            "filename match should rank first"
        );
        assert!(
            results[0].1.filename_bonus > 0,
            "username.rs should have filename bonus"
        );
        assert_eq!(
            results[1].1.filename_bonus, 0,
            "handler.rs should have no filename bonus"
        );
    }

    #[test]
    fn test_exact_filename_beats_fuzzy_filename() {
        // "username.rs" exactly matches "username.rs" → exact filename
        // "username.rs" is a fuzzy match of "user_name_handler.rs" → fuzzy bonus only
        let files = vec![
            make_file("src/user_name_handler.rs"),
            make_file("src/username.rs"),
        ];

        let results = search(&files, "username.rs");

        assert!(results.len() >= 2);
        assert_eq!(
            results[0].0, "src/username.rs",
            "exact filename should rank first"
        );
        assert_eq!(results[0].1.match_type, "exact_filename");
        assert!(results[0].1.filename_bonus > results[1].1.filename_bonus);
    }

    #[test]
    fn test_same_length_filename_no_false_exact() {
        // "item.rs" exactly matches "item.rs" → exact_filename
        // "item.rs" should NOT get exact_filename on "file.rs" even though stem lengths match
        let files = vec![
            make_file("src/item_sync/file.rs"),
            make_file("src/models/item.rs"),
        ];

        let results = search(&files, "item.rs");

        assert!(results.len() >= 2);
        assert_eq!(results[0].0, "src/models/item.rs");
        assert_eq!(results[0].1.match_type, "exact_filename");
        assert_ne!(
            results[1].1.match_type, "exact_filename",
            "file.rs should not get exact_filename"
        );
    }

    #[test]
    fn test_path_separator_disables_filename_bonus() {
        let files = vec![make_file("src/controllers/user.rs")];

        let results = search(&files, "src/user");

        assert!(!results.is_empty());
        assert_eq!(
            results[0].1.filename_bonus, 0,
            "path-like query should not get filename bonus"
        );
    }
}

#[cfg(test)]
mod multi_part_tests {
    #[test]
    fn test_single_path_matching() {
        let path = "core_workflow_service/kafka_event_consumer/src/ai_part_extraction_request/ai_part_extraction_request_handler.rs";

        // Test with max_typos = 2 (safe for short needles)
        let options = neo_frizbee::Config {
            max_typos: Some(2),
            sort: false,
            ..Default::default()
        };

        // Test "aipart" matching
        let matches = neo_frizbee::match_list("aipart", &[path], &options);
        println!("'aipart' matches (max_typos=2): {:?}", matches);
        assert!(!matches.is_empty(), "'aipart' should match the path");

        // Test "core" matching
        let matches = neo_frizbee::match_list("core", &[path], &options);
        println!("'core' matches (max_typos=2): {:?}", matches);
        assert!(!matches.is_empty(), "'core' should match the path");

        // Test "co" matching - need max_typos <= needle.len()
        let co_options = neo_frizbee::Config {
            max_typos: Some(2), // Safe: 2 <= len("co") = 2
            ..options
        };
        let matches = neo_frizbee::match_list("co", &[path], &co_options);
        println!("'co' matches (max_typos=2): {:?}", matches);
        assert!(!matches.is_empty(), "'co' should match the path");
    }

    #[test]
    fn test_lowercase_path_matching() {
        // The actual paths are lowercased
        let path = "core_workflow_service/kafka_event_consumer/src/ai_part_extraction_request/ai_part_extraction_request_handler.rs".to_lowercase();

        let options = neo_frizbee::Config {
            max_typos: Some(2),
            sort: false,
            ..Default::default()
        };

        // Test "co" matching on lowercase path
        let matches = neo_frizbee::match_list("co", &[path.as_str()], &options);
        println!("'co' matches lowercase path (max_typos=2): {:?}", matches);
        assert!(!matches.is_empty(), "'co' should match the lowercase path");

        // Test "core" matching on lowercase path
        let matches = neo_frizbee::match_list("core", &[path.as_str()], &options);
        println!("'core' matches lowercase path (max_typos=2): {:?}", matches);
        assert!(
            !matches.is_empty(),
            "'core' should match the lowercase path"
        );
    }
}
