//! High-performance grep engine for live content search.
//!
//! Searches file contents using the `grep-searcher` crate with mmap-backed
//! file access. Files are searched in frecency order for optimal pagination
//! performance — the most relevant files are searched first, enabling early
//! termination once enough results are collected.

use crate::constraints::apply_constraints;
use crate::mmap_cache::MmapCache;
use crate::sort_buffer::sort_with_buffer;
use crate::types::FileItem;
use fff_query_parser::{FFFQuery, FuzzyQuery, GrepConfig, QueryParser};
use grep_matcher::{Match, Matcher, NoCaptures, NoError};
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkMatch};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tracing::debug;

// ── Types ──────────────────────────────────────────────────────────────

/// A single content match within a file.
#[derive(Debug, Clone)]
pub struct GrepMatch {
    /// Index into the deduplicated `files` vec of the GrepResult.
    pub file_index: usize,
    /// 1-based line number.
    pub line_number: u64,
    /// 0-based byte column of first match start within the line.
    pub col: usize,
    /// Absolute byte offset of the matched line from the start of the file.
    /// Can be used by the preview to seek directly without scanning from the top.
    pub byte_offset: u64,
    /// The matched line text, truncated to `MAX_LINE_DISPLAY_LEN`.
    pub line_content: String,
    /// Byte offsets `(start, end)` within `line_content` for each match.
    pub match_byte_offsets: Vec<(u32, u32)>,
}

/// Result of a grep search.
#[derive(Debug, Clone, Default)]
pub struct GrepResult<'a> {
    pub matches: Vec<GrepMatch>,
    /// Deduplicated file references for the returned matches.
    pub files: Vec<&'a FileItem>,
    /// Total matches found before pagination cutoff.
    pub total_match_count: usize,
    /// Number of files actually searched (skips binary, too-large, etc.)
    pub total_files_searched: usize,
    /// Total number of indexed files.
    pub total_files: usize,
}

/// Options for grep search.
#[derive(Debug, Clone)]
pub struct GrepSearchOptions {
    pub max_file_size: u64,
    pub max_matches_per_file: usize,
    pub smart_case: bool,
    pub page_offset: usize,
    pub page_limit: usize,
}

// ── Regex Matcher (grep-matcher trait impl) ────────────────────────────

/// Lightweight wrapper around `regex::bytes::Regex` implementing the
/// `grep_matcher::Matcher` trait required by `grep-searcher`.
struct FffMatcher {
    regex: regex::bytes::Regex,
}

impl Matcher for FffMatcher {
    type Captures = NoCaptures;
    type Error = NoError;

    #[inline]
    fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<Match>, NoError> {
        Ok(self
            .regex
            .find_at(haystack, at)
            .map(|m| Match::new(m.start(), m.end())))
    }

    #[inline]
    fn new_captures(&self) -> Result<NoCaptures, NoError> {
        Ok(NoCaptures::new())
    }
}

// ── Sink (collects matches from searcher) ──────────────────────────────

/// Maximum bytes of a matched line to keep for display. Prevents minified
/// JS or huge single-line files from blowing up memory.
const MAX_LINE_DISPLAY_LEN: usize = 512;

/// Sink that collects `GrepMatch` entries from a single file search.
struct GrepSink<'r> {
    file_index: usize,
    matches: Vec<GrepMatch>,
    max_matches: usize,
    /// Regex for finding match positions within matched lines.
    regex: &'r regex::bytes::Regex,
}

impl Sink for GrepSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        // Check per-file limit
        if self.matches.len() >= self.max_matches {
            return Ok(false);
        }

        let line_bytes = mat.bytes();
        let line_number = mat.line_number().unwrap_or(0);
        let byte_offset = mat.absolute_byte_offset();

        let line_str = String::from_utf8_lossy(line_bytes);
        let trimmed = line_str.trim_end_matches(&['\n', '\r']);

        // Truncate for display (floor to a char boundary so we never split
        // a multi-byte UTF-8 sequence like Cyrillic, CJK, emoji, etc.)
        let display = if trimmed.len() > MAX_LINE_DISPLAY_LEN {
            let mut end = MAX_LINE_DISPLAY_LEN;
            while end > 0 && !trimmed.is_char_boundary(end) {
                end -= 1;
            }
            &trimmed[..end]
        } else {
            trimmed
        };
        let display_len = display.len() as u32;

        // Find all match positions within the display line — done here during
        // the initial search so we don't need a post-processing regex re-run.
        let mut match_byte_offsets = Vec::new();
        let mut col = 0usize;
        let mut first = true;

        for m in self.regex.find_iter(display.as_bytes()) {
            let start = (m.start() as u32).min(display_len);
            let end = (m.end() as u32).min(display_len);
            if first {
                col = m.start();
                first = false;
            }
            match_byte_offsets.push((start, end));
        }

        self.matches.push(GrepMatch {
            file_index: self.file_index,
            line_number,
            col,
            byte_offset,
            line_content: display.to_string(),
            match_byte_offsets,
        });

        Ok(true)
    }

    fn finish(
        &mut self,
        _searcher: &Searcher,
        _: &grep_searcher::SinkFinish,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Build a regex from the user's grep text.
/// - Escapes the input for literal matching (users type text, not regex)
/// - Applies smart case: case-insensitive unless query has uppercase
/// - Detects `\n` for multiline
fn build_regex(pattern: &str, smart_case: bool) -> Option<regex::bytes::Regex> {
    if pattern.is_empty() {
        return None;
    }

    // Check for multiline: user typed literal \n
    let (effective_pattern, _is_multiline) = if pattern.contains("\\n") {
        (pattern.replace("\\n", "\n"), true)
    } else {
        (pattern.to_string(), false)
    };

    // Escape for literal matching
    let escaped = regex::escape(&effective_pattern);

    // Smart case: case-insensitive unless query contains uppercase
    let case_insensitive = if smart_case {
        !pattern.chars().any(|c| c.is_uppercase())
    } else {
        false
    };

    regex::bytes::RegexBuilder::new(&escaped)
        .case_insensitive(case_insensitive)
        .unicode(false)
        .build()
        .ok()
}

// ── Main search function ───────────────────────────────────────────────

/// Perform a grep search across all indexed files.
///
/// When `query` is empty, returns git-modified/untracked files sorted by
/// frecency for the "welcome state" UI.
pub fn grep_search<'a>(
    files: &'a [FileItem],
    query: &str,
    parsed: Option<FFFQuery<'_>>,
    options: &GrepSearchOptions,
    mmap_cache: &MmapCache,
) -> GrepResult<'a> {
    let total_files = files.len();

    // Extract the grep text and file constraints from the parsed query.
    // For grep, the search pattern is the original query with constraint tokens
    // removed. This preserves spaces between consecutive text tokens:
    //   "name = *.rs someth" -> grep "name =" with constraint Extension("rs")
    //                           and a separate grep for "someth" is NOT done;
    //                           instead we produce the longest leading text run.
    //
    // Strategy: walk the original query's whitespace-split tokens in order.
    // Consecutive non-constraint tokens form text runs. We use the FIRST
    // (longest leading) text run as the grep pattern, since that's what the
    // user is most likely searching for. Constraints anywhere are extracted.
    // Trailing text after a constraint becomes a separate run and is ignored
    // for the grep pattern (it would make the regex disjunctive which grep
    // doesn't support).
    let constraints: &[fff_query_parser::Constraint<'_>];
    let extracted_grep_text: String;

    let grep_text: &str = match &parsed {
        Some(p) => {
            constraints = &p.constraints[..];
            if constraints.is_empty() {
                // No constraints at all — the entire query is the grep pattern.
                query.trim()
            } else {
                // Has constraints — rebuild grep text from the original query
                // by finding the first run of consecutive non-constraint tokens.
                extracted_grep_text = extract_first_text_run(query);
                &extracted_grep_text
            }
        }
        None => {
            constraints = &[];
            query.trim()
        }
    };

    // Empty query: return git-modified files sorted by frecency
    if grep_text.is_empty() {
        return build_empty_query_result(files, constraints, total_files);
    }

    // Build regex from the grep text
    let Some(regex) = build_regex(grep_text, options.smart_case) else {
        return GrepResult {
            total_files,
            ..Default::default()
        };
    };

    let matcher = FffMatcher {
        regex: regex.clone(),
    };

    // Determine if multiline mode is needed
    let is_multiline = grep_text.contains("\\n");

    // Filter files by constraints (reuses the existing constraint engine)
    let filtered: Vec<&FileItem> = if constraints.is_empty() {
        files
            .iter()
            .filter(|f| !f.is_binary && f.size > 0 && f.size <= options.max_file_size)
            .collect()
    } else {
        match apply_constraints(files, constraints) {
            Some(constrained) => constrained
                .into_iter()
                .filter(|f| !f.is_binary && f.size > 0 && f.size <= options.max_file_size)
                .collect(),
            None => files
                .iter()
                .filter(|f| !f.is_binary && f.size > 0 && f.size <= options.max_file_size)
                .collect(),
        }
    };

    // Sort by frecency descending for optimal pagination (best files first)
    let mut sorted_files = filtered;
    sort_with_buffer(&mut sorted_files, |a, b| {
        b.total_frecency_score
            .cmp(&a.total_frecency_score)
            .then(b.modified.cmp(&a.modified))
    });

    let target_count = options.page_offset + options.page_limit;
    let global_count = AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);

    debug!(
        grep_text,
        filtered_files = sorted_files.len(),
        target_count,
        is_multiline,
        "Starting grep search"
    );

    // Search files in parallel batches. We process in chunks to allow
    // early termination while still benefiting from parallelism.
    const BATCH_SIZE: usize = 256;
    let mut all_file_matches: Vec<(usize, Vec<GrepMatch>)> = Vec::new();
    let mut total_files_searched = 0usize;

    // Build a file_index mapping: we need stable indices into a dedup'd files vec
    // We'll assign indices post-search.

    for chunk in sorted_files.chunks(BATCH_SIZE) {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }

        let chunk_results: Vec<(usize, Vec<GrepMatch>)> = chunk
            .par_iter()
            .enumerate()
            .filter_map(|(chunk_idx, file)| {
                if cancelled.load(Ordering::Relaxed) {
                    return None;
                }

                let file_idx_in_all = total_files_searched + chunk_idx;

                // Get or create mmap
                let mmap = mmap_cache.get_or_insert(&file.path, file.size)?;

                let mut sink = GrepSink {
                    file_index: file_idx_in_all,
                    matches: Vec::new(),
                    max_matches: options.max_matches_per_file,
                    regex: &regex,
                };

                let mut searcher = SearcherBuilder::new()
                    .line_number(true)
                    .multi_line(is_multiline)
                    .build();

                // Search the mmap'd slice directly — no file I/O, no copies
                let _ = searcher.search_slice(&matcher, &mmap, &mut sink);

                if sink.matches.is_empty() {
                    None
                } else {
                    Some((file_idx_in_all, sink.matches))
                }
            })
            .collect();

        // Count matches from this batch
        let batch_match_count: usize = chunk_results.iter().map(|(_, m)| m.len()).sum();
        let prev_count = global_count.fetch_add(batch_match_count, Ordering::Relaxed);

        total_files_searched += chunk.len();
        all_file_matches.extend(chunk_results);

        // Stop processing more batches once we have enough matches for the
        // requested page. Each file that started in this batch ran to completion
        // (up to max_matches_per_file), so results are deterministic.
        if prev_count + batch_match_count >= target_count {
            cancelled.store(true, Ordering::Relaxed);
            break;
        }
    }

    let total_match_count = global_count.load(Ordering::Relaxed);

    // Build deduplicated files list and remap file indices
    let mut result_files: Vec<&'a FileItem> = Vec::new();

    // We need to map the chunk-relative file indices to the sorted_files indices
    // and then to deduplicated result indices
    let max_file_idx = all_file_matches
        .iter()
        .map(|(idx, _)| *idx)
        .max()
        .unwrap_or(0);
    let mut idx_remap = vec![usize::MAX; max_file_idx + 1];

    for (old_idx, _) in &all_file_matches {
        if idx_remap[*old_idx] == usize::MAX {
            let new_idx = result_files.len();
            idx_remap[*old_idx] = new_idx;
            result_files.push(sorted_files[*old_idx]);
        }
    }

    // Flatten all matches, remap file_index, paginate
    let mut all_matches: Vec<GrepMatch> = all_file_matches
        .into_iter()
        .flat_map(|(_, matches)| matches)
        .collect();

    // Remap file indices
    for m in &mut all_matches {
        m.file_index = idx_remap[m.file_index];
    }

    // Paginate
    let paginated = if options.page_offset < all_matches.len() {
        let end = (options.page_offset + options.page_limit).min(all_matches.len());
        all_matches[options.page_offset..end].to_vec()
    } else {
        Vec::new()
    };

    // Rebuild files vec to only include files referenced by paginated matches
    let mut final_files: Vec<&'a FileItem> = Vec::new();
    let mut final_remap = vec![usize::MAX; result_files.len()];
    let mut final_matches = paginated;

    for m in &mut final_matches {
        if final_remap[m.file_index] == usize::MAX {
            final_remap[m.file_index] = final_files.len();
            final_files.push(result_files[m.file_index]);
        }
        m.file_index = final_remap[m.file_index];
    }

    GrepResult {
        matches: final_matches,
        files: final_files,
        total_match_count,
        total_files_searched,
        total_files,
    }
}

/// Build the empty query result: git-modified/untracked files sorted by frecency.
/// This provides a useful "welcome state" showing files the user is actively working on.
fn build_empty_query_result<'a>(
    files: &'a [FileItem],
    constraints: &[fff_query_parser::Constraint<'_>],
    total_files: usize,
) -> GrepResult<'a> {
    use crate::git::is_modified_status;

    // Filter to git-modified/untracked files
    let mut changed_files: Vec<&FileItem> = if constraints.is_empty() {
        files
            .iter()
            .filter(|f| {
                f.git_status
                    .is_some_and(|s| is_modified_status(s) || s.contains(git2::Status::WT_NEW))
            })
            .collect()
    } else {
        match apply_constraints(files, constraints) {
            Some(constrained) => constrained
                .into_iter()
                .filter(|f| {
                    f.git_status
                        .is_some_and(|s| is_modified_status(s) || s.contains(git2::Status::WT_NEW))
                })
                .collect(),
            None => files
                .iter()
                .filter(|f| {
                    f.git_status
                        .is_some_and(|s| is_modified_status(s) || s.contains(git2::Status::WT_NEW))
                })
                .collect(),
        }
    };

    // Sort by frecency
    sort_with_buffer(&mut changed_files, |a, b| {
        b.total_frecency_score
            .cmp(&a.total_frecency_score)
            .then(b.modified.cmp(&a.modified))
    });

    // Limit to a reasonable number
    changed_files.truncate(50);

    let total_matched = changed_files.len();

    // For empty query, each file is a "match" with line_number = 0 (sentinel)
    let matches: Vec<GrepMatch> = changed_files
        .iter()
        .enumerate()
        .map(|(i, _)| GrepMatch {
            file_index: i,
            line_number: 0,
            col: 0,
            byte_offset: 0,
            line_content: String::new(),
            match_byte_offsets: Vec::new(),
        })
        .collect();

    GrepResult {
        matches,
        files: changed_files,
        total_match_count: total_matched,
        total_files_searched: 0,
        total_files,
    }
}

/// Extract the first consecutive run of non-constraint text tokens from a query.
///
/// Given `"name = *.rs someth"`, this returns `"name ="`:
/// - `"name"` → text (not a constraint)
/// - `"="` → text (not a constraint) → consecutive → extend run
/// - `"*.rs"` → constraint → stop the first run
/// - `"someth"` → text, but comes after a constraint → not part of the first run
///
/// This preserves the original spacing between consecutive text tokens by
/// tracking their byte positions in the source query.
fn extract_first_text_run(query: &str) -> String {
    let trimmed = query.trim();
    let mut first_run_start: Option<usize> = None;
    let mut first_run_end: usize = 0;

    for token in trimmed.split_whitespace() {
        let is_constraint = is_constraint_token(token);

        if is_constraint {
            // A constraint breaks the current text run
            if first_run_start.is_some() {
                // We already started a run — stop here
                break;
            }
            // Haven't started a text run yet — skip leading constraints
            continue;
        }

        // Find this token's byte position in the trimmed query
        let token_ptr = token.as_ptr() as usize;
        let base_ptr = trimmed.as_ptr() as usize;
        let token_offset = token_ptr - base_ptr;

        if first_run_start.is_none() {
            first_run_start = Some(token_offset);
        }
        first_run_end = token_offset + token.len();
    }

    match first_run_start {
        Some(start) if start < first_run_end => trimmed[start..first_run_end].to_string(),
        _ => String::new(),
    }
}

/// Quick check if a token looks like a grep constraint.
/// This mirrors the constraint patterns recognized by GrepConfig in the query parser.
#[inline]
fn is_constraint_token(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let bytes = token.as_bytes();
    match bytes[0] {
        // *.rs, *.toml — extension/glob constraints
        b'*' if token.len() > 2 && bytes[1] == b'.' => true,
        // /src/, /lib — path segment constraints
        b'/' if token.len() > 1 => true,
        // !*.rs, !/src/ — negation constraints
        b'!' if token.len() > 1 => {
            let inner = &token[1..];
            is_constraint_token(inner)
        }
        _ => {
            // Trailing slash: www/ — path segment
            if token.len() > 1 && bytes[bytes.len() - 1] == b'/' {
                return true;
            }
            // key:value — type:rust, status:modified, etc.
            if let Some(colon_pos) = token.find(':') {
                let key = &token[..colon_pos];
                matches!(key, "type" | "status" | "st" | "g" | "git")
            } else {
                false
            }
        }
    }
}

/// Parse a grep query using the GrepConfig parser.
pub fn parse_grep_query(query: &str) -> Option<FFFQuery<'_>> {
    let parser = QueryParser::new(GrepConfig);
    parser.parse(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_first_text_run_simple() {
        assert_eq!(extract_first_text_run("name"), "name");
    }

    #[test]
    fn test_extract_first_text_run_with_spaces() {
        assert_eq!(extract_first_text_run("name ="), "name =");
    }

    #[test]
    fn test_extract_first_text_run_with_constraint_after() {
        assert_eq!(extract_first_text_run("name = *.rs"), "name =");
    }

    #[test]
    fn test_extract_first_text_run_with_constraint_and_trailing_text() {
        assert_eq!(extract_first_text_run("name = *.rs someth"), "name =");
    }

    #[test]
    fn test_extract_first_text_run_leading_constraint() {
        assert_eq!(extract_first_text_run("*.rs name ="), "name =");
    }

    #[test]
    fn test_extract_first_text_run_only_constraints() {
        assert_eq!(extract_first_text_run("*.rs /src/"), "");
    }

    #[test]
    fn test_extract_first_text_run_empty() {
        assert_eq!(extract_first_text_run(""), "");
    }

    #[test]
    fn test_extract_first_text_run_path_constraint() {
        assert_eq!(extract_first_text_run("name /src/ value"), "name");
    }

    #[test]
    fn test_extract_first_text_run_negation() {
        assert_eq!(extract_first_text_run("name !*.rs value"), "name");
    }

    #[test]
    fn test_is_constraint_token() {
        assert!(is_constraint_token("*.rs"));
        assert!(is_constraint_token("/src/"));
        assert!(is_constraint_token("/lib"));
        assert!(is_constraint_token("www/"));
        assert!(is_constraint_token("!*.rs"));
        assert!(is_constraint_token("!/src/"));
        assert!(is_constraint_token("type:rust"));
        assert!(is_constraint_token("status:modified"));

        assert!(!is_constraint_token("name"));
        assert!(!is_constraint_token("="));
        assert!(!is_constraint_token("fn"));
        assert!(!is_constraint_token("hello:world")); // unknown key
    }
}
