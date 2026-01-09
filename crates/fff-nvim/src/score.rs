use crate::{
    git::is_modified_status,
    path_utils::calculate_distance_penalty,
    sort_buffer::{sort_by_key_with_buffer, sort_with_buffer},
    types::{FileItem, Score, ScoringContext},
};
use fff_query_parser::{Constraint, FuzzyQuery, GitStatusFilter, QueryParser};
use neo_frizbee::Scoring;
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::MAIN_SEPARATOR;
use zlob::{ZlobFlags, zlob_match_paths};

/// Precompute glob matches for all patterns, returning sets of matching path indices
fn precompute_glob_matches<'a>(
    constraints: &[Constraint<'a>],
    paths: &[&str],
) -> Vec<(bool, HashSet<usize>)> {
    let mut results = Vec::new();
    collect_glob_indices(constraints, paths, &mut results, false);
    results
}

fn collect_glob_indices<'a>(
    constraints: &[Constraint<'a>],
    paths: &[&str],
    results: &mut Vec<(bool, HashSet<usize>)>,
    is_negated: bool,
) {
    for constraint in constraints {
        match constraint {
            Constraint::Glob(pattern) => {
                if let Ok(Some(matches)) = zlob_match_paths(pattern, paths, ZlobFlags::RECOMMENDED) {
                    // Convert matched paths to indices
                    let matched_set: HashSet<&str> = matches.into_iter().collect();
                    let indices: HashSet<usize> = paths
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| matched_set.contains(*p))
                        .map(|(i, _)| i)
                        .collect();
                    results.push((is_negated, indices));
                } else {
                    // No matches - empty set
                    results.push((is_negated, HashSet::new()));
                }
            }
            Constraint::Not(inner) => {
                collect_glob_indices(&[inner.as_ref().clone()], paths, results, !is_negated);
            }
            _ => {}
        }
    }
}

/// Check if a file at given index matches a constraint (single-pass friendly)
#[inline]
fn file_matches_constraint_at_index(
    file: &FileItem,
    file_index: usize,
    constraint: &Constraint<'_>,
    glob_results: &[(bool, HashSet<usize>)],
    glob_idx: &mut usize,
    negate: bool,
) -> bool {
    let matches = match constraint {
        Constraint::Extension(ext) => {
            let suffix = format!(".{}", ext);
            file.file_name.ends_with(&suffix)
        }
        Constraint::Glob(_) => {
            // Use precomputed glob results
            let result = glob_results.get(*glob_idx).map(|(is_neg, set)| {
                let matched = set.contains(&file_index);
                if *is_neg { !matched } else { matched }
            }).unwrap_or(true);
            *glob_idx += 1;
            return if negate { !result } else { result };
        }
        Constraint::PathSegment(segment) => {
            let segment_with_slashes = format!("/{}/", segment);
            let segment_prefix = format!("{}/", segment);
            file.relative_path.contains(&segment_with_slashes)
                || file.relative_path.starts_with(&segment_prefix)
        }
        Constraint::GitStatus(status_filter) => {
            match (file.git_status, status_filter) {
                (Some(status), GitStatusFilter::Modified) => is_modified_status(status),
                (Some(status), GitStatusFilter::Untracked) => status.contains(git2::Status::WT_NEW),
                (Some(status), GitStatusFilter::Staged) => {
                    status.intersects(
                        git2::Status::INDEX_NEW
                            | git2::Status::INDEX_MODIFIED
                            | git2::Status::INDEX_DELETED
                            | git2::Status::INDEX_RENAMED
                            | git2::Status::INDEX_TYPECHANGE,
                    )
                }
                (Some(status), GitStatusFilter::Unmodified) => status.is_empty(),
                (None, GitStatusFilter::Unmodified) => true,
                (None, _) => false,
            }
        }
        Constraint::Not(inner) => {
            return file_matches_constraint_at_index(file, file_index, inner, glob_results, glob_idx, !negate);
        }
        Constraint::Text(text) => {
            let text_lower = text.to_lowercase();
            file.relative_path_lower.contains(&text_lower)
        }
        // Parts and Exclude are handled at a higher level
        Constraint::Parts(_) | Constraint::Exclude(_) | Constraint::FileType(_) => true,
    };
    
    if negate { !matches } else { matches }
}

/// Apply constraint-based prefiltering in a single pass over all files.
/// Returns a vector of indices into the original files slice that pass all constraints.
/// Only applies constraints when there are 2+ components in the query.
fn apply_constraints(
    files: &[FileItem],
    constraints: &[Constraint<'_>],
    token_count: usize,
) -> Option<Vec<usize>> {
    if token_count < 2 || constraints.is_empty() {
        return None;
    }

    // Collect paths for glob matching (zlob is more efficient matching all paths at once)
    let paths: Vec<&str> = files.iter().map(|f| f.relative_path.as_str()).collect();
    
    // Precompute glob matches
    let glob_results = precompute_glob_matches(constraints, &paths);

    // Single pass: check all constraints for each file
    let indices: Vec<usize> = files
        .iter()
        .enumerate()
        .filter(|(i, file)| {
            let mut glob_idx = 0;
            // File must match ALL constraints
            constraints.iter().all(|constraint| {
                file_matches_constraint_at_index(file, *i, constraint, &glob_results, &mut glob_idx, false)
            })
        })
        .map(|(i, _)| i)
        .collect();

    Some(indices)
}

/// Match files against all fuzzy parts.
/// Single part: use optimized batch matching.
/// Multiple parts: each part must match, scores are summed (Nucleo-style).
fn match_fuzzy_parts(
    fuzzy_parts: &[&str],
    working_files: &[&FileItem],
    options: &neo_frizbee::Config,
) -> Vec<neo_frizbee::Match> {
    if fuzzy_parts.is_empty() {
        return vec![];
    }

    let haystack: Vec<&str> = working_files
        .iter()
        .map(|f| f.relative_path_lower.as_str())
        .collect();

    // Single part - use optimized batch matching (original behavior)
    if fuzzy_parts.len() == 1 {
        return neo_frizbee::match_list(fuzzy_parts[0], &haystack, options);
    }

    // Multiple parts - match first part, then filter by remaining parts
    let mut matches = neo_frizbee::match_list(fuzzy_parts[0], &haystack, options);
    
    for part in &fuzzy_parts[1..] {
        matches = matches
            .into_iter()
            .filter_map(|mut m| {
                let path = haystack.get(m.index as usize)?;
                let part_matches = neo_frizbee::match_list(part, &[*path], options);
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

    let parsed = parser.parse(context.query);
    let token_count = context.query.split_whitespace().count();

    let (working_files, _file_indices): (Vec<&FileItem>, Option<Vec<usize>>) =
        match parsed.as_ref().and_then(|p| {
            if p.constraints.is_empty() {
                None
            } else {
                apply_constraints(files, &p.constraints, token_count)
            }
        }) {
            Some(indices) if !indices.is_empty() => {
                let filtered: Vec<&FileItem> = indices.iter().map(|&i| &files[i]).collect();
                tracing::debug!(
                    "Prefiltered {} files to {} using {} constraints",
                    files.len(),
                    filtered.len(),
                    parsed.as_ref().map(|p| p.constraints.len()).unwrap_or(0)
                );
                (filtered, Some(indices))
            }
            Some(_) => {
                tracing::debug!("All files filtered out by constraints");
                return (vec![], vec![], 0);
            }
            None => {
                (files.iter().collect(), None)
            }
        };

    let query_trimmed = context.query.trim();
    let fuzzy_parts: &[&str] = match &parsed {
        None => {
            if query_trimmed.len() < 2 {
                return score_filtered_by_frecency(&working_files, context);
            }
            std::slice::from_ref(&query_trimmed)
        }
        Some(p) => match &p.fuzzy_query {
            FuzzyQuery::Text(t) if t.len() >= 2 => std::slice::from_ref(t),
            FuzzyQuery::Parts(parts) if !parts.is_empty() => parts.as_slice(),
            _ => return score_filtered_by_frecency(&working_files, context),
        },
    };

    let has_uppercase = fuzzy_parts.iter().any(|p| p.chars().any(|c| c.is_uppercase()));
    let query_contains_path_separator = fuzzy_parts.iter().any(|p| p.contains(MAIN_SEPARATOR));
    
    let options = neo_frizbee::Config {
        prefilter: true,
        max_typos: Some(context.max_typos),
        sort: false,
        scoring: Scoring {
            capitalization_bonus: if has_uppercase { 8 } else { 0 },
            matching_case_bonus: if has_uppercase { 4 } else { 0 },
            ..Default::default()
        },
    };

    let path_matches = match_fuzzy_parts(fuzzy_parts, &working_files, &options);
    
    tracing::debug!(
        "Fuzzy match {:?}: {} results",
        fuzzy_parts,
        path_matches.len()
    );

    let primary_text = fuzzy_parts[0]; // Use first part for filename matching
    let haystack_of_filenames: Vec<&str> = path_matches
        .iter()
        .filter_map(|m| {
            working_files
                .get(m.index as usize)
                .map(|f| f.file_name_lower.as_str())
        })
        .collect();

    // if there is a / in the query we don't even match filenames
    let filename_matches = if query_contains_path_separator {
        vec![]
    } else {
        // Use parallel matching only if we have enough filenames to justify overhead
        // Sequential matching is faster for small result sets (< 1000 matches)
        let mut list = if haystack_of_filenames.len() > 1000 {
            neo_frizbee::match_list_parallel(
                primary_text,
                &haystack_of_filenames,
                &options,
                context.max_threads,
            )
        } else {
            neo_frizbee::match_list(primary_text, &haystack_of_filenames, &options)
        };

        // Sequential sort is faster for small lists
        if list.len() > 1000 {
            list.par_sort_unstable_by_key(|m| m.index);
        } else {
            sort_by_key_with_buffer(&mut list, |m| m.index);
        }

        list
    };

    let mut next_filename_match_index = 0;
    let results: Vec<_> = path_matches
        .into_iter()
        .enumerate()
        .map(|(index, path_match)| {
            let file_idx = path_match.index as usize;
            let file = working_files[file_idx];

            let mut base_score = path_match.score as i32;
            let frecency_boost = base_score.saturating_mul(file.total_frecency_score as i32) / 100;
            let distance_penalty =
                calculate_distance_penalty(context.current_file, &file.relative_path);

            let filename_match = filename_matches
                .get(next_filename_match_index)
                .and_then(|m| {
                    if m.index == index as u32 {
                        next_filename_match_index += 1;
                        Some(m)
                    } else {
                        None
                    }
                });

            let mut has_special_filename_bonus = false;
            let filename_bonus = match filename_match {
                Some(filename_match) if filename_match.exact => {
                    filename_match.score as i32 / 5 * 2 // 40% bonus for exact filename match
                }
                // 16% bonus for fuzzy filename match but only if the score of matched path is
                // equal or greater than the score of matched filename, thus we are not allowing
                // typoed filename to score higher than the path match
                Some(filename_match)
                    if filename_match.score >= path_match.score
                        && !query_contains_path_separator =>
                {
                    base_score = filename_match.score as i32;

                    (base_score / 6)
                        // for large queries around ~300 score the bonus is too big
                        // it might lead to situations when much more fitting path with a larger
                        // base score getting filtered out by combination of score + filename bonus
                        // so we cap it at 10% of the roughly largest score you can get
                        .min(30)
                }
                // 5% bonus for special file but not as much as file name to avoid sitatuions
                // when you have /user_service/server.rs and /user_service/server/mod.rs
                None if is_special_entry_point_file(&file.file_name) => {
                    has_special_filename_bonus = true;
                    base_score * 5 / 100
                }
                _ => 0,
            };

            let current_file_penalty = calculate_current_file_penalty(file, base_score, context);

            let combo_match_boost = {
                let last_same_query_match = context
                    .last_same_query_match
                    .filter(|m| m.file_path.as_os_str() == file.path.as_os_str());

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
                distance_penalty,
                combo_match_boost,
                exact_match: path_match.exact || filename_match.is_some_and(|m| m.exact),
                match_type: match filename_match {
                    Some(filename_match) if filename_match.exact => "exact_filename",
                    Some(_) => "fuzzy_filename",
                    None => "fuzzy_path",
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
fn score_filtered_by_frecency<'a>(
    files: &[&'a FileItem],
    context: &ScoringContext,
) -> (Vec<&'a FileItem>, Vec<Score>, usize) {
    let results: Vec<_> = files
        .par_iter()
        .map(|&file| {
            let total_frecency_score = file.access_frecency_score as i32
                + (file.modification_frecency_score as i32).saturating_mul(4);

            let current_file_penalty =
                calculate_current_file_penalty(file, total_frecency_score, context);
            let total = total_frecency_score.saturating_add(current_file_penalty);

            let score = Score {
                total,
                base_score: 0,
                filename_bonus: 0,
                distance_penalty: 0,
                special_filename_bonus: 0,
                combo_match_boost: 0,
                current_file_penalty,
                frecency_boost: total_frecency_score,
                exact_match: false,
                match_type: "frecency",
            };

            (file, score)
        })
        .collect();

    sort_and_paginate(results, context)
}

fn score_all_by_frecency<'a>(
    files: &'a [FileItem],
    context: &ScoringContext,
) -> (Vec<&'a FileItem>, Vec<Score>, usize) {
    let results: Vec<_> = files
        .par_iter()
        .map(|file| {
            let total_frecency_score = file.access_frecency_score as i32
                + (file.modification_frecency_score as i32).saturating_mul(4);

            let current_file_penalty =
                calculate_current_file_penalty(file, total_frecency_score, context);
            let total = total_frecency_score.saturating_add(current_file_penalty);

            let score = Score {
                total,
                base_score: 0,
                filename_bonus: 0,
                distance_penalty: 0,
                special_filename_bonus: 0,
                combo_match_boost: 0,
                current_file_penalty,
                frecency_boost: total_frecency_score,
                exact_match: false,
                match_type: "frecency",
            };

            (file, score)
        })
        .collect();

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
        && file.relative_path.as_str() == current
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
    use std::path::PathBuf;

    fn create_test_file(path: &str, score: i32, modified: u64) -> (FileItem, Score) {
        let file = FileItem {
            path: PathBuf::from(path),
            relative_path: path.to_string(),
            relative_path_lower: path.to_lowercase(),
            file_name: path.split('/').last().unwrap_or(path).to_string(),
            file_name_lower: path.split('/').last().unwrap_or(path).to_lowercase(),
            size: 0,
            modified,
            access_frecency_score: 0,
            modification_frecency_score: 0,
            total_frecency_score: 0,
            git_status: None,
        };
        let score_obj = Score {
            total: score,
            base_score: score,
            filename_bonus: 0,
            distance_penalty: 0,
            special_filename_bonus: 0,
            current_file_penalty: 0,
            frecency_boost: 0,
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

        let context = ScoringContext {
            query: "test",
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
        assert_eq!(items[0].relative_path, "file4.rs");
        assert_eq!(items[1].relative_path, "file6.rs");
        assert_eq!(items[2].relative_path, "file2.rs");
    }

    #[test]
    fn test_partial_sort_with_same_scores() {
        // Test tiebreaker with modified time
        let test_data = vec![
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

        let context = ScoringContext {
            query: "test",
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
        let test_data = vec![
            create_test_file("file1.rs", 100, 1000),
            create_test_file("file2.rs", 200, 2000),
            create_test_file("file3.rs", 50, 3000),
        ];

        let results: Vec<(&FileItem, Score)> = test_data
            .iter()
            .map(|(file, score)| (file, score.clone()))
            .collect();

        let context = ScoringContext {
            query: "test",
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
        assert_eq!(items[0].relative_path, "file2.rs");
        assert_eq!(items[1].relative_path, "file1.rs");
        assert_eq!(items[2].relative_path, "file3.rs");
    }
}
