use std::path::MAIN_SEPARATOR;

use crate::{
    git::is_modified_status,
    path_utils::calculate_distance_penalty,
    types::{FileItem, Score, ScoringContext},
};
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

    let options = neo_frizbee::Options {
        prefilter: true,
        max_typos: Some(context.max_typos),
        sort: false,
    };

    let query_contains_path_separator = context.query.contains(MAIN_SEPARATOR);
    let haystack: Vec<&str> = files.iter().map(|f| f.relative_path.as_str()).collect();
    tracing::debug!(
        "Starting fuzzy search for query '{}' in {} files",
        context.query,
        haystack.len()
    );
    let path_matches = neo_frizbee::match_list(context.query, &haystack, options);
    tracing::debug!(
        "Matched {} files for query '{}'",
        path_matches.len(),
        context.query
    );

    // assume that filename should only match if the path matches
    // we should actually incorporate this bonus by getting this information from neo_frizbee directly
    // instead of spawning a separate matching process, but it's okay for the beta
    let haystack_of_filenames = path_matches
        .par_iter()
        .filter_map(|m| {
            files
                .get(m.index_in_haystack as usize)
                .map(|f| f.file_name.as_str())
        })
        .collect::<Vec<_>>();

    // if there is a / in the query we don't even match filenames
    let filename_matches = if query_contains_path_separator {
        vec![]
    } else {
        let mut list = neo_frizbee::match_list_parallel(
            context.query,
            &haystack_of_filenames,
            options,
            context.max_threads,
        );

        list.par_sort_unstable_by_key(|m| m.index_in_haystack);

        list
    };

    let mut next_filename_match_index = 0;
    let results: Vec<_> = path_matches
        .into_iter()
        .enumerate()
        .map(|(index, path_match)| {
            let file_idx = path_match.index_in_haystack as usize;
            let file = &files[file_idx];

            let mut base_score = path_match.score as i32;
            let frecency_boost = base_score.saturating_mul(file.total_frecency_score as i32) / 100;
            let distance_penalty =
                calculate_distance_penalty(context.current_file, &file.relative_path);

            let filename_match = filename_matches
                .get(next_filename_match_index)
                .and_then(|m| {
                    if m.index_in_haystack == index as u32 {
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

    if let Some(current) = context.current_file {
        if file.relative_path.as_str() == current {
            penalty -= match file.git_status {
                Some(status) if is_modified_status(status) => base_score / 2,
                _ => base_score,
            };

            tracing::debug!(file =?file.relative_path, current=?context.current_file, ?penalty, "Calculating current file penalty");
        }
    }

    penalty
}

/// Dynamically sorts and returns the top results either in ascending or descending order
fn sort_and_truncate<'a>(
    mut results: Vec<(&'a FileItem, Score)>,
    context: &ScoringContext,
) -> (Vec<&'a FileItem>, Vec<Score>, usize) {
    let total_matched = results.len();
    if context.reverse_order {
        results.sort_by(|a, b| {
            a.1.total
                .cmp(&b.1.total)
                .then_with(|| a.0.modified.cmp(&b.0.modified))
        });

        if results.len() > context.max_results {
            results.drain(0..(total_matched - context.max_results));
        }
    } else {
        results.sort_by(|a, b| {
            b.1.total
                .cmp(&a.1.total)
                .then_with(|| b.0.modified.cmp(&a.0.modified))
        });

        results.truncate(context.max_results);
    }
    let (items, scores) = results.into_iter().unzip();
    (items, scores, total_matched)
}
