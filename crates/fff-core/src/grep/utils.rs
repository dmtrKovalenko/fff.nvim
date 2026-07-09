use super::grep::{GrepMatch, GrepSearchOptions};
use crate::types::FileItem;

#[inline]
pub(crate) fn strip_line_terminators(bytes: &[u8]) -> &[u8] {
    let mut len = bytes.len();
    while len > 0 && matches!(bytes[len - 1], b'\n' | b'\r') {
        len -= 1;
    }
    &bytes[..len]
}

/// Result of a grep search with a list of matches, list of matched files, and metadata.
#[derive(Debug, Clone, Default)]
pub struct GrepResult<'a> {
    pub matches: Vec<GrepMatch>,
    /// Deduplicated file references for the returned matches.
    pub files: Vec<&'a FileItem>,
    /// Number of files actually searched in this call.
    pub total_files_searched: usize,
    /// Total number of indexed files (before filtering).
    pub total_files: usize,
    /// Total number of searchable files (after filtering out binary, too-large, etc.).
    pub filtered_file_count: usize,
    /// Number of files that contained at least one match.
    pub files_with_matches: usize,
    /// The file offset to pass for the next page. `0` if there are no more files.
    /// Callers should store this and pass it as `file_offset` in the next call.
    pub next_file_offset: usize,
    /// When regex mode fails to compile the pattern, the search falls back to
    /// literal matching and this field contains the compilation error message.
    /// The UI can display this to inform the user their regex was invalid.
    pub regex_fallback_error: Option<String>,
}

impl<'a> GrepResult<'a> {
    /// Empty result carrying only the file counts (empty query / prefilter miss)
    pub(crate) fn empty(total_files: usize, filtered_file_count: usize) -> Self {
        Self {
            total_files,
            filtered_file_count,
            ..Default::default()
        }
    }

    pub(crate) fn collect(
        per_file_results: Vec<(usize, &'a FileItem, Vec<GrepMatch>)>,
        files_to_search_len: usize,
        options: &GrepSearchOptions,
        total_files: usize,
        filtered_file_count: usize,
        budget_exceeded: bool,
    ) -> Self {
        let page_limit = options.page_limit;

        // Each match stores a `file_index` pointing into `result_files` so that
        // consumers (FFI JSON, Lua) can look up file metadata without duplicating
        // it across every match from the same file
        let mut result_files: Vec<&'a FileItem> = Vec::new();
        let mut all_matches: Vec<GrepMatch> = Vec::new();
        // files_consumed tracks how far into files_to_search we have advanced,
        // counting every file whose results were emitted (with or without matches).
        // We use the batch_idx of the last consumed file + 1, which is correct
        // because per_file_results only contains files that had matches, and
        // files between them that had no matches were still searched and can be
        // safely skipped on the next page
        let mut files_consumed: usize = 0;

        for (batch_idx, file, file_matches) in per_file_results {
            // batch_idx is the 0-based position in files_to_search.
            // Advance files_consumed to include this file and all no-match files before it.
            files_consumed = batch_idx + 1;

            let file_result_idx = result_files.len();
            result_files.push(file);

            for mut m in file_matches {
                m.file_index = file_result_idx;
                if options.trim_whitespace {
                    m.trim_leading_whitespace();
                }
                all_matches.push(m);
            }

            // page_limit is a soft cap: we always finish the current file before
            // stopping, so no matches are dropped. A page may return up to
            // page_limit + max_matches_per_file - 1 matches in the worst case
            if all_matches.len() >= page_limit {
                break;
            }
        }

        // If no file had any match, we searched the entire slice.
        if result_files.is_empty() {
            files_consumed = files_to_search_len;
        }

        let has_more = budget_exceeded
            || (all_matches.len() >= page_limit && files_consumed < files_to_search_len);

        let next_file_offset = if has_more {
            options.file_offset + files_consumed
        } else {
            0
        };

        Self {
            matches: all_matches,
            files_with_matches: result_files.len(),
            files: result_files,
            total_files_searched: files_consumed,
            total_files,
            filtered_file_count,
            next_file_offset,
            regex_fallback_error: None,
        }
    }
}

pub fn has_regex_metacharacters(text: &str) -> bool {
    regex::escape(text) != text
}
