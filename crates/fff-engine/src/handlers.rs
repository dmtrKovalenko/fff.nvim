use fff_ipc::types::{FindOptions, GrepOptions, SearchResponse, WireGrepMode};

use crate::state::EngineState;

pub async fn handle_grep(
    state: &EngineState,
    query: String,
    options: GrepOptions,
) -> SearchResponse {
    use fff::{AiGrepConfig, QueryParser};
    use fff_ipc::types::WireGrepResponse;

    let picker_arc = state.shared_picker.clone();
    let result = tokio::task::spawn_blocking(move || {
        let guard = picker_arc.read().map_err(|e| e.to_string())?;
        let picker = guard
            .as_ref()
            .ok_or_else(|| "File picker not yet initialized".to_string())?;

        let grep_options = to_core_grep_options(&options);

        let parser = QueryParser::new(AiGrepConfig);
        let parsed = parser.parse(&query);
        let result = picker.grep(&parsed, &grep_options);
        let wire_matches = project_grep_result(&result, picker);

        Ok::<_, String>(WireGrepResponse {
            matches: wire_matches,
            total_files_searched: result.total_files_searched,
            total_files: result.total_files,
            files_with_matches: result.files_with_matches,
            next_file_offset: result.next_file_offset,
            regex_fallback_error: result.regex_fallback_error,
        })
    })
    .await;

    match result {
        Ok(Ok(wire)) => SearchResponse::GrepResults(wire),
        Ok(Err(msg)) => SearchResponse::Error(msg),
        Err(e) => SearchResponse::Error(format!("spawn_blocking join error: {e}")),
    }
}

pub async fn handle_find_files(
    state: &EngineState,
    query: String,
    options: FindOptions,
) -> SearchResponse {
    use fff::{FuzzySearchOptions, PaginationArgs, QueryParser};
    use fff_ipc::types::WireSearchResult;

    let picker_arc = state.shared_picker.clone();
    let result = tokio::task::spawn_blocking(move || {
        let guard = picker_arc.read().map_err(|e| e.to_string())?;
        let picker = guard
            .as_ref()
            .ok_or_else(|| "File picker not yet initialized".to_string())?;

        let parser = QueryParser::default();
        let parsed = parser.parse(&query);

        let current_file_str = options.current_file.clone();
        let search_result = picker.fuzzy_search(
            &parsed,
            None,
            FuzzySearchOptions {
                max_threads: options.max_threads,
                current_file: current_file_str.as_deref(),
                project_path: None,
                combo_boost_score_multiplier: options.combo_boost_score_multiplier,
                min_combo_count: options.min_combo_count,
                pagination: PaginationArgs {
                    offset: options.offset,
                    limit: options.limit,
                },
            },
        );

        let wire: Vec<WireSearchResult> = search_result
            .items
            .iter()
            .zip(search_result.scores.iter())
            .map(|(item, score)| WireSearchResult {
                path: item.relative_path(picker),
                score: score.total,
                git_status: item.git_status.map(|s| s.bits()),
                frecency_score: item.total_frecency_score(),
            })
            .collect();

        Ok::<_, String>(wire)
    })
    .await;

    match result {
        Ok(Ok(wire)) => SearchResponse::SearchResults(wire),
        Ok(Err(msg)) => SearchResponse::Error(msg),
        Err(e) => SearchResponse::Error(format!("spawn_blocking join error: {e}")),
    }
}

pub async fn handle_multi_grep(
    state: &EngineState,
    patterns: Vec<String>,
    constraints: Option<String>,
    options: GrepOptions,
) -> SearchResponse {
    use fff::{AiGrepConfig, QueryParser};
    use fff_ipc::types::WireGrepResponse;

    let picker_arc = state.shared_picker.clone();
    let result = tokio::task::spawn_blocking(move || {
        let guard = picker_arc.read().map_err(|e| e.to_string())?;
        let picker = guard
            .as_ref()
            .ok_or_else(|| "File picker not yet initialized".to_string())?;

        let grep_options = to_core_grep_options(&options);

        let parser = QueryParser::new(AiGrepConfig);
        let constraint_query = constraints.as_deref().unwrap_or("");
        let parsed_constraints = parser.parse(constraint_query);

        let patterns_refs: Vec<&str> = patterns.iter().map(|s| s.as_str()).collect();
        let result = picker.multi_grep(
            &patterns_refs,
            parsed_constraints.constraints.as_slice(),
            &grep_options,
        );

        let wire_matches = project_grep_result(&result, picker);

        Ok::<_, String>(WireGrepResponse {
            matches: wire_matches,
            total_files_searched: result.total_files_searched,
            total_files: result.total_files,
            files_with_matches: result.files_with_matches,
            next_file_offset: result.next_file_offset,
            regex_fallback_error: result.regex_fallback_error,
        })
    })
    .await;

    match result {
        Ok(Ok(wire)) => SearchResponse::GrepResults(wire),
        Ok(Err(msg)) => SearchResponse::Error(msg),
        Err(e) => SearchResponse::Error(format!("spawn_blocking join error: {e}")),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn to_core_grep_options(options: &GrepOptions) -> fff::grep::GrepSearchOptions {
    fff::grep::GrepSearchOptions {
        max_file_size: options.max_file_size,
        max_matches_per_file: options.max_matches_per_file,
        smart_case: options.smart_case,
        file_offset: options.file_offset,
        page_limit: options.page_limit,
        mode: wire_mode_to_grep_mode(options.mode),
        time_budget_ms: options.time_budget_ms,
        before_context: options.before_context,
        after_context: options.after_context,
        classify_definitions: options.classify_definitions,
        trim_whitespace: options.trim_whitespace,
        abort_signal: None,
    }
}

/// Project a `GrepResult` into owned wire types while the picker read-guard is held.
///
/// `FileItem.path` is a `ChunkedString` (arena-relative pointer) that becomes
/// invalid once the guard drops, so this must be called inside `spawn_blocking`
/// with the picker still borrowed.
fn project_grep_result(
    result: &fff::grep::GrepResult<'_>,
    picker: &fff::file_picker::FilePicker,
) -> Vec<fff_ipc::types::WireGrepFileMatches> {
    use std::collections::HashMap;
    use fff_ipc::types::{WireGrepFileMatches, WireGrepMatch};

    let mut by_file: HashMap<usize, WireGrepFileMatches> = HashMap::new();
    for m in &result.matches {
        let file = result.files[m.file_index];
        let entry = by_file.entry(m.file_index).or_insert_with(|| {
            WireGrepFileMatches {
                path: file.relative_path(picker),
                size: file.size,
                git_status: file.git_status.map(|s| s.bits()),
                frecency_score: file.total_frecency_score(),
                matches: Vec::new(),
            }
        });
        entry.matches.push(WireGrepMatch {
            line_number: m.line_number,
            col: m.col,
            line_text: m.line_content.clone(),
            match_byte_offsets: m.match_byte_offsets.iter().copied().collect(),
            is_definition: m.is_definition,
            context_before: m.context_before.clone(),
            context_after: m.context_after.clone(),
        });
    }
    // Preserve file ordering from result.files.
    let mut ordered: Vec<WireGrepFileMatches> = Vec::new();
    for i in 0..result.files.len() {
        if let Some(fm) = by_file.remove(&i) {
            ordered.push(fm);
        }
    }
    ordered
}

fn wire_mode_to_grep_mode(mode: WireGrepMode) -> fff::grep::GrepMode {
    match mode {
        WireGrepMode::PlainText => fff::grep::GrepMode::PlainText,
        WireGrepMode::Regex => fff::grep::GrepMode::Regex,
        WireGrepMode::Fuzzy => fff::grep::GrepMode::Fuzzy,
    }
}
