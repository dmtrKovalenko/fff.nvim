//! Output formatting for MCP grep/search results.
//!
//! Port of `packages/fff-mcp/src/output.ts` — token-efficient formatting
//! with definition auto-expansion, frecency/git annotations, and Read suggestions.

use fff_core::GrepMatch;
use fff_core::git::format_git_status;
use fff_core::grep::is_import_line;
use fff_core::types::FileItem;

use crate::cursor::CursorStore;

/// Frecency score → single-token word. Empty for low-scoring files.
fn frecency_word(score: i64) -> &'static str {
    if score >= 100 {
        "hot"
    } else if score >= 50 {
        "warm"
    } else if score >= 10 {
        "frequent"
    } else {
        ""
    }
}

/// Git status → single-token word. Empty for clean files.
fn git_word(status: &str) -> &'static str {
    match status {
        "modified" => "modified",
        "untracked" => "untracked",
        "added" | "staged_new" => "staged",
        "deleted" => "deleted",
        "renamed" => "renamed",
        "conflicted" => "conflicted",
        _ => "",
    }
}

/// Build " - hot git:modified" style suffix. Empty when nothing to report.
pub fn file_suffix(git_status: Option<git2::Status>, frecency_score: i64) -> String {
    let git_str = format_git_status(git_status);
    let f = frecency_word(frecency_score);
    let g = git_word(git_str);
    if f.is_empty() && g.is_empty() {
        return String::new();
    }
    let mut parts = Vec::new();
    if !f.is_empty() {
        parts.push(f.to_string());
    }
    if !g.is_empty() {
        parts.push(format!("git:{}", g));
    }
    format!(" - {}", parts.join(" "))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Content,
    FilesWithMatches,
    Count,
    Usage,
}

impl OutputMode {
    pub fn new(s: Option<&str>) -> Self {
        match s {
            Some("files_with_matches") => Self::FilesWithMatches,
            Some("count") => Self::Count,
            Some("usage") => Self::Usage,
            _ => Self::Content,
        }
    }
}

const LARGE_FILE_BYTES: u64 = 20_000;

/// Tag for large files — nudges model to use offset/limit when reading.
fn size_tag(bytes: u64) -> String {
    if bytes < LARGE_FILE_BYTES {
        String::new()
    } else {
        let kb = (bytes + 512) / 1024; // round
        format!(" ({}KB - use offset to read relevant section)", kb)
    }
}

const MAX_PREVIEW: usize = 120;
const MAX_LINE_LEN: usize = 180;
/// Max context lines to show when auto-expanding the first definition
const MAX_DEF_EXPAND_FIRST: usize = 8;
/// Max context lines for subsequent definitions
const MAX_DEF_EXPAND: usize = 5;
/// Max context lines for non-definition first match in small result sets
const MAX_FIRST_MATCH_EXPAND: usize = 8;

/// Truncate a line centered on the match region.
/// Shows 1/3 context before match, match itself, then remaining budget after.
fn trunc_centered(line: &str, match_ranges: Option<&[(u32, u32)]>, max_len: usize) -> String {
    if line.len() <= max_len {
        return line.to_string();
    }

    // Use first match range to center the window
    if let Some(ranges) = match_ranges {
        if let Some(&(match_start, match_end)) = ranges.first() {
            let match_start = match_start as usize;
            let match_end = match_end as usize;
            let match_len = match_end.saturating_sub(match_start);

            let budget = max_len.saturating_sub(match_len);
            let before = budget / 3;
            let after = budget - before;

            let win_start = match_start.saturating_sub(before);
            let win_end = (match_end + after).min(line.len());

            // Clamp to char boundaries
            let win_start = floor_char_boundary(line, win_start);
            let win_end = ceil_char_boundary(line, win_end);

            let mut result = line[win_start..win_end].to_string();
            if win_start > 0 {
                result.insert(0, '…');
            }
            if win_end < line.len() {
                result.push('…');
            }
            return result;
        }
    }

    // No match ranges — truncate from start
    let end = ceil_char_boundary(line, max_len);
    format!("{}…", &line[..end])
}

/// Floor to a valid char boundary
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Ceil to a valid char boundary
fn ceil_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Collected file metadata for the first match per file.
struct FileMeta<'a> {
    file: &'a FileItem,
    line_number: u64,
    line_content: String,
    is_definition: bool,
    match_ranges: Vec<(u32, u32)>,
    context_after: Vec<String>,
}

/// Parameters for [`format_grep_results`].
///
/// Groups the read-only inputs so callers don't juggle 10 positional args.
pub struct FormatGrepParams<'a> {
    pub matches: &'a [GrepMatch],
    pub files: &'a [&'a FileItem],
    pub total_matched: usize,
    pub next_file_offset: usize,
    pub regex_fallback_error: Option<&'a str>,
    pub output_mode: OutputMode,
    pub max_results: usize,
    pub show_context: bool,
    pub auto_expand_defs: bool,
}

/// Format grep results into token-efficient text output.
pub fn format_grep_results(
    params: &FormatGrepParams<'_>,
    cursor_store: &mut CursorStore,
) -> String {
    let FormatGrepParams {
        matches,
        files,
        total_matched,
        next_file_offset,
        regex_fallback_error,
        output_mode,
        max_results,
        show_context,
        auto_expand_defs,
    } = *params;
    let items = if matches.len() > max_results {
        &matches[..max_results]
    } else {
        matches
    };

    if output_mode == OutputMode::FilesWithMatches {
        return format_files_with_matches(
            items,
            files,
            next_file_offset,
            auto_expand_defs,
            cursor_store,
        );
    }

    if output_mode == OutputMode::Count {
        return format_count(items, files, next_file_offset, cursor_store);
    }

    // usage
    let mut lines: Vec<String> = Vec::new();
    let unique_files = {
        let mut seen = std::collections::HashSet::new();
        for m in items {
            seen.insert(m.file_index);
        }
        seen.len()
    };
    let max_output_chars: usize = if output_mode == OutputMode::Usage {
        5000
    } else if unique_files <= 3 {
        5000
    } else if unique_files <= 8 {
        3500
    } else {
        2500
    };

    if let Some(err) = regex_fallback_error {
        lines.push(format!("! regex failed: {}, using literal match", err));
    }

    // File overview: collect first match per file
    let file_preview = collect_file_preview(items, files);

    // Find best Read target: prefer [def] file, fallback to first
    let mut content_def_file = "";
    let mut content_first_file = "";
    for fm in &file_preview {
        if content_first_file.is_empty() {
            content_first_file = &fm.file.relative_path;
        }
        if content_def_file.is_empty() && fm.is_definition {
            content_def_file = &fm.file.relative_path;
        }
    }
    let content_suggest = if !content_def_file.is_empty() {
        content_def_file
    } else {
        content_first_file
    };
    if !content_suggest.is_empty() {
        let file_count = file_preview.len();
        if file_count == 1 {
            lines.push(format!("→ Read {} (only match)", content_suggest));
        } else if !content_def_file.is_empty() {
            lines.push(format!("→ Read {} [def]", content_suggest));
        } else if file_count <= 3 {
            lines.push(format!("→ Read {} (best match)", content_suggest));
        }
    }

    if total_matched > items.len() {
        lines.push(format!("{}/{} matches shown", items.len(), total_matched));
    }

    // Track which files already had a definition expanded
    let mut def_expanded_files = std::collections::HashSet::new();

    // Detailed content (subject to budget)
    let mut char_count = 0usize;
    let mut shown_count = 0usize;
    let mut current_file = "";

    // Reorder: definitions first, then usages, then imports (when auto-expanding)
    let sorted_items: Vec<usize> = if auto_expand_defs {
        let mut indices: Vec<usize> = (0..items.len()).collect();
        indices.sort_unstable_by_key(|&i| {
            if items[i].is_definition {
                0
            } else if is_import_line(&items[i].line_content) {
                2
            } else {
                1
            }
        });

        indices
    } else {
        (0..items.len()).collect()
    };

    for &idx in &sorted_items {
        let m = &items[idx];
        let file = files[m.file_index];
        let mut match_lines: Vec<String> = Vec::new();

        if file.relative_path.as_str() != current_file {
            current_file = &file.relative_path;
            match_lines.push(current_file.to_string());
        }

        // Skip import-only lines when we already have definitions
        if auto_expand_defs && is_import_line(&m.line_content) && !def_expanded_files.is_empty() {
            continue;
        }

        // Context before (only when explicitly requested)
        if show_context && !m.context_before.is_empty() {
            let start_line = m.line_number.saturating_sub(m.context_before.len() as u64);
            for (i, ctx) in m.context_before.iter().enumerate() {
                match_lines.push(format!(
                    " {}-{}",
                    start_line + i as u64,
                    trunc_centered(ctx, None, MAX_LINE_LEN)
                ));
            }
        }

        // Match line
        match_lines.push(format!(
            " {}: {}",
            m.line_number,
            trunc_centered(&m.line_content, Some(&m.match_byte_offsets.as_ref()), MAX_LINE_LEN)
        ));

        // Context after (only when explicitly requested via context parameter)
        if show_context && !m.context_after.is_empty() {
            let start_line = m.line_number + 1;
            for (i, ctx) in m.context_after.iter().enumerate() {
                match_lines.push(format!(
                    " {}-{}",
                    start_line + i as u64,
                    trunc_centered(ctx, None, MAX_LINE_LEN)
                ));
            }
            match_lines.push("--".to_string());
        }

        // Auto-expand definitions with body context
        if auto_expand_defs
            && !show_context
            && m.is_definition
            && !m.context_after.is_empty()
            && !def_expanded_files.contains(file.relative_path.as_str())
        {
            let expand_limit = if def_expanded_files.is_empty() {
                MAX_DEF_EXPAND_FIRST
            } else {
                MAX_DEF_EXPAND
            };
            def_expanded_files.insert(file.relative_path.as_str());
            let start_line = m.line_number + 1;
            for (i, ctx) in m.context_after.iter().take(expand_limit).enumerate() {
                if ctx.trim().is_empty() {
                    break;
                }
                match_lines.push(format!(
                    "  {}| {}",
                    start_line + i as u64,
                    trunc_centered(ctx, None, MAX_LINE_LEN)
                ));
            }
        }

        let chunk = match_lines.join("\n");
        if char_count + chunk.len() > max_output_chars && shown_count > 0 {
            break;
        }

        char_count += chunk.len();
        lines.push(chunk);
        shown_count += 1;
    }

    if next_file_offset > 0 {
        let cursor_id = cursor_store.store(next_file_offset);
        lines.push(format!("\ncursor: {}", cursor_id));
    }

    lines.join("\n")
}

fn format_files_with_matches(
    items: &[GrepMatch],
    files: &[&FileItem],
    next_file_offset: usize,
    auto_expand_defs: bool,
    cursor_store: &mut CursorStore,
) -> String {
    let file_map = collect_file_preview(items, files);

    let mut lines: Vec<String> = Vec::new();
    let file_count = file_map.len();

    // Find best Read target
    let mut first_def_file = "";
    let mut first_file = "";
    for fm in &file_map {
        if first_file.is_empty() {
            first_file = &fm.file.relative_path;
        }
        if first_def_file.is_empty() && fm.is_definition {
            first_def_file = &fm.file.relative_path;
        }
    }
    let suggest_path = if !first_def_file.is_empty() {
        first_def_file
    } else {
        first_file
    };

    if !suggest_path.is_empty() {
        if file_count == 1 {
            lines.push(format!(
                "→ Read {} (only match — no need to search further)",
                suggest_path
            ));
        } else if !first_def_file.is_empty() && file_count <= 5 {
            lines.push(format!("→ Read {} (definition found)", suggest_path));
        } else if !first_def_file.is_empty() {
            lines.push(format!("→ Read {} (definition)", suggest_path));
        } else if file_count <= 3 {
            lines.push(format!("→ Read {} (best match)", suggest_path));
        } else {
            lines.push(format!("→ Read {}", suggest_path));
        }
    }

    let is_small_set = file_count <= 5;
    let mut def_expanded_count = 0usize;

    for (file_idx, fm) in file_map.iter().enumerate() {
        let is_def = fm.is_definition;
        let def_tag = if is_def { " [def]" } else { "" };
        lines.push(format!(
            "{}{}{}",
            fm.file.relative_path,
            def_tag,
            size_tag(fm.file.size)
        ));

        // Show preview
        if !fm.line_content.is_empty() && (is_def || file_idx == 0 || is_small_set) {
            let ranges_ref: Option<&[(u32, u32)]> = if fm.match_ranges.is_empty() {
                None
            } else {
                Some(&fm.match_ranges)
            };
            lines.push(format!(
                "  {}: {}",
                fm.line_number,
                trunc_centered(&fm.line_content, ranges_ref, MAX_PREVIEW)
            ));

            // Auto-expand body context
            if auto_expand_defs && !fm.context_after.is_empty() {
                let expand_limit = if is_def {
                    let limit = if def_expanded_count == 0 {
                        MAX_DEF_EXPAND_FIRST
                    } else {
                        MAX_DEF_EXPAND
                    };
                    def_expanded_count += 1;
                    limit
                } else if is_small_set && file_idx == 0 {
                    MAX_FIRST_MATCH_EXPAND
                } else if is_small_set {
                    MAX_DEF_EXPAND
                } else {
                    0
                };

                if expand_limit > 0 {
                    let start_line = fm.line_number + 1;
                    for (i, ctx) in fm.context_after.iter().take(expand_limit).enumerate() {
                        if ctx.trim().is_empty() {
                            break;
                        }
                        lines.push(format!(
                            "  {}| {}",
                            start_line + i as u64,
                            trunc_centered(ctx, None, MAX_PREVIEW)
                        ));
                    }
                }
            }
        }
    }

    if next_file_offset > 0 {
        let cursor_id = cursor_store.store(next_file_offset);
        lines.push(format!("\ncursor: {}", cursor_id));
    }

    lines.join("\n")
}

fn format_count(
    items: &[GrepMatch],
    files: &[&FileItem],
    next_file_offset: usize,
    cursor_store: &mut CursorStore,
) -> String {
    let mut count_map: Vec<(&str, usize)> = Vec::new();
    for m in items {
        let file = files[m.file_index];
        if let Some(entry) = count_map
            .iter_mut()
            .find(|(p, _)| *p == file.relative_path.as_str())
        {
            entry.1 += 1;
        } else {
            count_map.push((&file.relative_path, 1));
        }
    }

    let mut lines: Vec<String> = Vec::new();
    for (path, count) in &count_map {
        lines.push(format!("{}: {}", path, count));
    }
    if next_file_offset > 0 {
        let cursor_id = cursor_store.store(next_file_offset);
        lines.push(format!("\ncursor: {}", cursor_id));
    }
    lines.join("\n")
}

fn collect_file_preview<'a>(items: &[GrepMatch], files: &[&'a FileItem]) -> Vec<FileMeta<'a>> {
    let mut file_preview: Vec<FileMeta<'a>> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for m in items {
        let file = files[m.file_index];
        if seen.insert(&file.relative_path) {
            file_preview.push(FileMeta {
                file,
                line_number: m.line_number,
                line_content: m.line_content.clone(),
                is_definition: m.is_definition,
                match_ranges: m.match_byte_offsets.iter().copied().collect(),
                context_after: m.context_after.clone(),
            });
        }
    }
    file_preview
}
