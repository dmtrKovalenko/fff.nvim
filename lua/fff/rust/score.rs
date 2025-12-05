use std::path::MAIN_SEPARATOR;

use crate::{
    git::is_modified_status,
    path_utils::calculate_distance_penalty,
    sort_buffer::{sort_by_key_with_buffer, sort_with_buffer},
    types::{FileItem, Score, ScoringContext},
};
use neo_frizbee::Scoring;
use rayon::prelude::*;

pub fn match_and_score_files<'a>(
    files: &'a [FileItem],
    context: &ScoringContext,
) -> (Vec<&'a FileItem>, Vec<Score>, usize) {
    if context.query.len() < 2 {
        return score_all_by_frecency(files, context);
    }

    if files.is_empty() {
        return (vec![], vec![], 0);
    }

    let has_uppercase_letter = context.query.chars().any(|c| c.is_uppercase());
    let options = neo_frizbee::Config {
        prefilter: true,
        max_typos: Some(context.max_typos),
        sort: false,
        scoring: Scoring {
            capitalization_bonus: if has_uppercase_letter { 8 } else { 0 },
            matching_case_bonus: if has_uppercase_letter { 4 } else { 0 },
            ..Default::default()
        },
    };

    let query_contains_path_separator = context.query.contains(MAIN_SEPARATOR);
    let haystack: Vec<&str> = files
        .iter()
        .map(|f| f.relative_path_lower.as_str())
        .collect();
    tracing::debug!(
        "Starting fuzzy search for query '{}' in {} files",
        context.query,
        haystack.len()
    );
    let path_matches = neo_frizbee::match_list(context.query, &haystack, &options);
    tracing::debug!(
        "Matched {} files for query '{}'",
        path_matches.len(),
        context.query
    );

    // assume that filename should only match if the path matches
    // we should actually incorporate this bonus by getting this information from neo_frizbee directly
    // instead of spawning a separate matching process, but it's okay for the beta
    // Use sequential iteration - this is a simple filtering operation that's faster without Rayon overhead
    let haystack_of_filenames: Vec<&str> = path_matches
        .iter()
        .filter_map(|m| {
            files
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
                context.query,
                &haystack_of_filenames,
                &options,
                context.max_threads,
            )
        } else {
            neo_frizbee::match_list(context.query, &haystack_of_filenames, &options)
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
            let file = &files[file_idx];

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
            if current_file_penalty < 0 {
                tracing::debug!(file =?file.relative_path, ?current_file_penalty, "Applied penalty");
            }

            let total = base_score
                .saturating_add(frecency_boost)
                .saturating_add(distance_penalty)
                .saturating_add(filename_bonus)
                .saturating_add(current_file_penalty);

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

    sort_and_truncate(results, context)
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
                current_file_penalty,
                frecency_boost: total_frecency_score,
                exact_match: false,
                match_type: "frecency",
            };

            (file, score)
        })
        .collect();

    sort_and_truncate(results, context)
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

        tracing::debug!(file =?file.relative_path, current=?context.current_file, ?penalty, "Calculating current file penalty");
    }

    penalty
}

/// Dynamically sorts and returns the top results either in ascending or descending order
/// Uses partial sorting for large result sets to improve performance
fn sort_and_truncate<'a>(
    mut results: Vec<(&'a FileItem, Score)>,
    context: &ScoringContext,
) -> (Vec<&'a FileItem>, Vec<Score>, usize) {
    let total_matched = results.len();

    // For large result sets, use partial sort to avoid sorting everything
    let threshold = context.max_results * 2;

    if context.reverse_order {
        // Ascending order: want highest N items displayed as [low -> high]
        if results.len() > threshold {
            // Partition at position (len - max_results) with ascending comparator
            // This puts the highest max_results items after this position
            let partition_index = results.len() - context.max_results;
            results.select_nth_unstable_by(partition_index, |a, b| {
                a.1.total
                    .cmp(&b.1.total)
                    .then_with(|| a.0.modified.cmp(&b.0.modified))
            });
            // Remove everything before partition_index, keeping highest max_results items
            results.drain(0..partition_index);
        }

        // Sort remaining results in ascending order using glidesort
        sort_with_buffer(&mut results, |a, b| {
            a.1.total
                .cmp(&b.1.total)
                .then_with(|| a.0.modified.cmp(&b.0.modified))
        });

        // If still more than max_results (for small datasets), drain the front
        if results.len() > context.max_results {
            results.drain(0..(results.len() - context.max_results));
        }
    } else {
        // Descending order: want highest N items displayed as [high -> low]
        if results.len() > threshold {
            // Partition at position (max_results - 1) with descending comparator
            // This puts the highest max_results items at the front
            results.select_nth_unstable_by(context.max_results - 1, |a, b| {
                b.1.total
                    .cmp(&a.1.total)
                    .then_with(|| b.0.modified.cmp(&a.0.modified))
            });
            // Keep only the first max_results items
            results.truncate(context.max_results);
        }

        // Sort remaining results in descending order using glidesort
        sort_with_buffer(&mut results, |a, b| {
            b.1.total
                .cmp(&a.1.total)
                .then_with(|| b.0.modified.cmp(&a.0.modified))
        });

        // Ensure we only return max_results items (for small datasets)
        results.truncate(context.max_results);
    }

    let (items, scores) = results.into_iter().unzip();
    (items, scores, total_matched)
}

#[cfg(test)]
mod tests {
    use super::*;
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
            max_results: 3,
            max_threads: 1,
            max_typos: 2,
            current_file: None,
            reverse_order: false,
        };

        // Test with partial sort (threshold = 3 * 2 = 6, our len is 10 > 6)
        let (items, scores, total) = sort_and_truncate(results.clone(), &context);

        // Should return top 3: 300, 250, 200
        assert_eq!(total, 10);
        assert_eq!(scores.len(), 3);
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
            max_results: 3,
            max_threads: 1,
            max_typos: 2,
            current_file: None,
            reverse_order: false,
        };

        let (items, scores, _) = sort_and_truncate(results, &context);

        // Should return: 200(9000), 200(1000), 100(8000)
        assert_eq!(scores[0].total, 200);
        assert_eq!(items[0].modified, 9000, "First 200 should be newest");
        assert_eq!(scores[1].total, 200);
        assert_eq!(items[1].modified, 1000, "Second 200 should be older");
        assert_eq!(scores[2].total, 100);
        assert_eq!(items[2].modified, 8000, "Third should be newest of 100s");
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
            max_results: 2,
            max_threads: 1,
            max_typos: 2,
            current_file: None,
            reverse_order: false,
        };

        // threshold = 2 * 2 = 4, len = 3 < 4, so regular sort
        let (items, scores, _) = sort_and_truncate(results, &context);

        assert_eq!(scores.len(), 2);
        assert_eq!(scores[0].total, 200);
        assert_eq!(scores[1].total, 100);
        assert_eq!(items[0].relative_path, "file2.rs");
        assert_eq!(items[1].relative_path, "file1.rs");
    }

    #[test]
    fn test_reverse_order_partial_sort() {
        let test_data = vec![
            create_test_file("file1.rs", 100, 1000),
            create_test_file("file2.rs", 200, 2000),
            create_test_file("file3.rs", 50, 3000),
            create_test_file("file4.rs", 300, 4000),
            create_test_file("file5.rs", 150, 5000),
            create_test_file("file6.rs", 250, 6000),
        ];

        let results: Vec<(&FileItem, Score)> = test_data
            .iter()
            .map(|(file, score)| (file, score.clone()))
            .collect();

        let context = ScoringContext {
            query: "test",
            max_results: 3,
            max_threads: 1,
            max_typos: 2,
            current_file: None,
            reverse_order: true,
        };

        let (items, scores, _) = sort_and_truncate(results, &context);

        // Reverse order should return highest 3 in ascending order: 200, 250, 300
        // This is for bottom-prompt UI where best results appear at bottom near the prompt
        assert_eq!(scores.len(), 3);
        assert_eq!(scores[0].total, 200, "First should be third highest");
        assert_eq!(scores[1].total, 250, "Second should be second highest");
        assert_eq!(scores[2].total, 300, "Third should be highest");
        assert_eq!(items[0].relative_path, "file2.rs");
        assert_eq!(items[1].relative_path, "file6.rs");
        assert_eq!(items[2].relative_path, "file4.rs");
    }
}
