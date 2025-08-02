use crate::{
    git::is_modified_status,
    path_utils::calculate_distance_penalty,
    types::{FileItem, Score, ScoringContext},
};
use rayon::prelude::*;

pub fn match_and_score_files(files: &[FileItem], context: &ScoringContext) -> Vec<(usize, Score)> {
    if context.query.len() < 2 {
        return score_all_by_frecency(files, context);
    }

    if files.is_empty() {
        return Vec::new();
    }

    let options = neo_frizbee::Options {
        prefilter: true,
        max_typos: Some(context.max_typos),
        sort: false,
    };

    let haystack: Vec<&str> = files.iter().map(|f| f.relative_path.as_str()).collect();
    tracing::debug!(
        "Starting fuzzy search for query '{}' in {} files",
        context.query,
        haystack.len()
    );
    let path_matches =
        neo_frizbee::match_list_parallel(context.query, &haystack, options, context.max_threads);
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

    let mut filename_matches = neo_frizbee::match_list_parallel(
        context.query,
        &haystack_of_filenames,
        options,
        context.max_threads,
    );

    filename_matches.par_sort_by_key(|m| m.index_in_haystack);
    let mut next_filename_match_index = 0;
    let mut results: Vec<_> = path_matches
        .into_iter()
        .enumerate()
        .map(|(index, neo_frizbee_match)| {
            let file_idx = neo_frizbee_match.index_in_haystack as usize;
            let file = &files[file_idx];

            let base_score = neo_frizbee_match.score as i32;
            let frecency_boost = base_score.saturating_mul(file.total_frecency_score as i32) / 100;
            let distance_penalty = calculate_distance_penalty(
                context.current_file.map(|s| s.as_str()),
                &file.relative_path,
            );

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

            tracing::debug!(filename_match = ?filename_match, "Filename match for file {}", index);

            let mut has_special_filename_bonus = false;
            let filename_bonus = match filename_match {
                Some(filename_match) if filename_match.exact => {
                    base_score / 5 * 2 // 40% bonus for exact filename match
                }
                Some(_) => base_score / 5, // 20% bonus for fuzzy filename match
                None if is_special_entry_point_file(&file.file_name) => {
                    // 18% bonus special filename just as much as exact path
                    // but a little bit less to give preference to the actual file if present
                    has_special_filename_bonus = true;
                    base_score * 18 / 100
                }
                None => 0,
            };

            let total = base_score
                .saturating_add(frecency_boost)
                .saturating_add(distance_penalty)
                .saturating_add(filename_bonus);

            let score = Score {
                total,
                base_score,
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

            (file_idx, score)
        })
        .collect();

    results.par_sort_by(|a, b| b.1.total.cmp(&a.1.total));

    results
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

fn score_all_by_frecency(files: &[FileItem], context: &ScoringContext) -> Vec<(usize, Score)> {
    files
        .par_iter()
        .enumerate()
        .map(|(idx, file)| {
            let total_frecency_score = file.access_frecency_score as i32
                + (file.modification_frecency_score as i32).saturating_mul(4);

            let distance_penalty = calculate_distance_penalty(
                context.current_file.map(|x| x.as_str()),
                &file.relative_path,
            );

            let total = total_frecency_score
                .saturating_add(distance_penalty)
                .saturating_add(calculate_file_bonus(file, context));

            let score = Score {
                total,
                base_score: 0,
                filename_bonus: 0,
                special_filename_bonus: 0,
                frecency_boost: total_frecency_score,
                distance_penalty,
                match_type: "frecency",
            };

            (idx, score)
        })
        .collect()
}

#[inline]
fn calculate_file_bonus(file: &FileItem, context: &ScoringContext) -> i32 {
    let mut bonus = 0i32;

    if let Some(current) = context.current_file {
        if file.relative_path == *current {
            bonus -= match file.git_status {
                Some(status) if is_modified_status(status) => 150,
                _ => 300,
            };
        }
    }

    bonus
}
