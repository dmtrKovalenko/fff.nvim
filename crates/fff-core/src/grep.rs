//! High-performance grep engine for live content search.
//!
//! Searches file contents using the `grep-searcher` crate with mmap-backed
//! file access. Files are searched in frecency order for optimal pagination
//! performance — the most relevant files are searched first, enabling early
//! termination once enough results are collected.

use crate::constraints::apply_constraints;
use crate::sort_buffer::sort_with_buffer;
use crate::types::FileItem;
use fff_query_parser::{FFFQuery, GrepConfig, QueryParser};
use grep_matcher::{Match, Matcher, NoCaptures, NoError};
use grep_searcher::lines::{self, LineStep};
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkMatch};
use rayon::prelude::*;
use smallvec::SmallVec;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::Level;

/// Controls how the grep pattern is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrepMode {
    /// Default mode: the query is treated as literal text.
    /// The pattern is searched using SIMD-accelerated `memchr::memmem`.
    /// Special regex characters in the query have no special meaning.
    #[default]
    PlainText,
    /// Regex mode: the query is treated as a regular expression.
    /// Uses the same `grep-matcher` / `regex::bytes::Regex` engine.
    /// Invalid regex patterns will return zero results (not an error).
    Regex,
    /// Fuzzy mode: the query is treated as a fuzzy needle matched against
    /// each line using neo_frizbee's Smith-Waterman scoring. Lines are ranked
    /// by match score. Individual matched character positions are reported
    /// as highlight ranges.
    Fuzzy,
}

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
    /// Stack-allocated for the common case of ≤4 spans per line.
    pub match_byte_offsets: SmallVec<[(u32, u32); 4]>,
    /// Fuzzy match score from neo_frizbee (only set in Fuzzy grep mode).
    pub fuzzy_score: Option<u16>,
}

/// Result of a grep search.
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
    /// The file offset to pass for the next page. `0` if there are no more files.
    /// Callers should store this and pass it as `file_offset` in the next call.
    pub next_file_offset: usize,
    /// When regex mode fails to compile the pattern, the search falls back to
    /// literal matching and this field contains the compilation error message.
    /// The UI can display this to inform the user their regex was invalid.
    pub regex_fallback_error: Option<String>,
}

/// Options for grep search.
#[derive(Debug, Clone)]
pub struct GrepSearchOptions {
    pub max_file_size: u64,
    pub max_matches_per_file: usize,
    pub smart_case: bool,
    /// File-based pagination offset: index into the sorted/filtered file list
    /// to start searching from. Pass 0 for the first page, then use
    /// `GrepResult::next_file_offset` for subsequent pages.
    pub file_offset: usize,
    /// Maximum number of matches to collect before stopping.
    pub page_limit: usize,
    /// How to interpret the search pattern. Defaults to `PlainText`.
    pub mode: GrepMode,
    /// Maximum time in milliseconds to spend searching before returning partial
    /// results. Prevents UI freezes on pathological queries. 0 = no limit.
    pub time_budget_ms: u64,
}

/// Lightweight wrapper around `regex::bytes::Regex` implementing the
/// `grep_matcher::Matcher` trait required by `grep-searcher`.
///
/// When `is_multiline` is false (the common case), we report `\n` as the
/// line terminator. This enables the **fast** search path in `fff-searcher`:
/// instead of calling `shortest_match()` on every single line (slow path),
/// the searcher calls `find_candidate_line()` once on the entire remaining
/// buffer, letting the regex DFA skip non-matching content in a single pass.
///
/// For multiline patterns we must NOT report a line terminator — the regex
/// can match across line boundaries, so the searcher needs the `MultiLine`
/// strategy.
struct RegexMatcher<'r> {
    regex: &'r regex::bytes::Regex,
    is_multiline: bool,
}

impl Matcher for RegexMatcher<'_> {
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

    #[inline]
    fn line_terminator(&self) -> Option<grep_matcher::LineTerminator> {
        if self.is_multiline {
            None
        } else {
            Some(grep_matcher::LineTerminator::byte(b'\n'))
        }
    }
}

/// A `grep_matcher::Matcher` backed by `memchr::memmem` for literal search.
///
/// This is used in `PlainText` mode and is significantly faster than regex
/// for literal patterns: memchr uses SIMD (AVX2/NEON) two-way substring
/// search internally, avoiding the overhead of regex compilation and DFA
/// state transitions.
///
/// Always reports `\n` as line terminator so the searcher uses the fast
/// candidate-line path (plain text can never span lines unless `\n` is
/// literally in the needle, which we handle separately).
struct PlainTextMatcher {
    /// Case-folded needle bytes for case-insensitive matching.
    /// When case-sensitive, this is the original pattern bytes.
    needle: Vec<u8>,
    case_insensitive: bool,
}

impl Matcher for PlainTextMatcher {
    type Captures = NoCaptures;
    type Error = NoError;

    #[inline]
    fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<Match>, NoError> {
        let hay = &haystack[at..];

        let found = if self.case_insensitive {
            // ASCII case-insensitive: lowercase the haystack slice on the fly.
            // We scan with a rolling window to avoid allocating a full copy.
            ascii_case_insensitive_find(hay, &self.needle)
        } else {
            memchr::memmem::find(hay, &self.needle)
        };

        Ok(found.map(|pos| Match::new(at + pos, at + pos + self.needle.len())))
    }

    #[inline]
    fn new_captures(&self) -> Result<NoCaptures, NoError> {
        Ok(NoCaptures::new())
    }

    #[inline]
    fn line_terminator(&self) -> Option<grep_matcher::LineTerminator> {
        Some(grep_matcher::LineTerminator::byte(b'\n'))
    }
}

/// ASCII case-insensitive substring search.
///
/// Lowercases only the first byte of the needle for the initial scan using
/// memchr, then compares the rest byte-by-byte with ASCII lowering.
/// This avoids allocating a lowered copy of the haystack.
#[inline]
fn ascii_case_insensitive_find(haystack: &[u8], needle_lower: &[u8]) -> Option<usize> {
    if needle_lower.is_empty() {
        return Some(0);
    }
    if haystack.len() < needle_lower.len() {
        return None;
    }

    let first = needle_lower[0]; // already lowered
    let first_upper = first.to_ascii_uppercase();

    // Use memchr2 to find positions of either case of the first byte.
    // When the first byte is non-alphabetic both variants are the same,
    // memchr2 handles that efficiently.
    for pos in memchr::memchr2_iter(first, first_upper, haystack) {
        if pos + needle_lower.len() > haystack.len() {
            return None;
        }
        let candidate = &haystack[pos..pos + needle_lower.len()];
        if candidate
            .iter()
            .zip(needle_lower.iter())
            .all(|(&h, &n)| h.to_ascii_lowercase() == n)
        {
            return Some(pos);
        }
    }
    None
}

/// Maximum bytes of a matched line to keep for display. Prevents minified
/// JS or huge single-line files from blowing up memory.
const MAX_LINE_DISPLAY_LEN: usize = 512;

struct SinkState {
    file_index: usize,
    matches: Vec<GrepMatch>,
    max_matches: usize,
}

impl SinkState {
    #[inline]
    fn prepare_line<'a>(line_bytes: &'a [u8], mat: &SinkMatch<'_>) -> (&'a [u8], u32, u64, u64) {
        let line_number = mat.line_number().unwrap_or(0);
        let byte_offset = mat.absolute_byte_offset();

        // Trim trailing newline/CR directly on bytes to avoid UTF-8 conversion.
        let trimmed_len = {
            let mut len = line_bytes.len();
            while len > 0 && matches!(line_bytes[len - 1], b'\n' | b'\r') {
                len -= 1;
            }
            len
        };
        let trimmed_bytes = &line_bytes[..trimmed_len];

        // Truncate for display (floor to a char boundary).
        let display_bytes = if trimmed_bytes.len() > MAX_LINE_DISPLAY_LEN {
            let mut end = MAX_LINE_DISPLAY_LEN;
            while end > 0 && !is_utf8_char_boundary(trimmed_bytes[end]) {
                end -= 1;
            }
            &trimmed_bytes[..end]
        } else {
            trimmed_bytes
        };

        let display_len = display_bytes.len() as u32;
        (display_bytes, display_len, line_number, byte_offset)
    }

    #[inline]
    fn push_match(
        &mut self,
        line_number: u64,
        col: usize,
        byte_offset: u64,
        line_content: String,
        match_byte_offsets: SmallVec<[(u32, u32); 4]>,
    ) {
        self.matches.push(GrepMatch {
            file_index: self.file_index,
            line_number,
            col,
            byte_offset,
            line_content,
            match_byte_offsets,
            fuzzy_score: None,
        });
    }
}

/// Sink for `PlainText` mode.
///
/// Highlights are extracted with SIMD-accelerated `memchr::memmem::Finder`.
/// Case-insensitive matching lowercases the line into a stack buffer before
/// searching, keeping positions 1:1 for ASCII.
/// No regex engine is involved at any point.
struct PlainTextSink<'r> {
    state: SinkState,
    finder: &'r memchr::memmem::Finder<'r>,
    pattern_len: u32,
    case_insensitive: bool,
}

impl Sink for PlainTextSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        if self.state.max_matches != 0 && self.state.matches.len() >= self.state.max_matches {
            return Ok(false);
        }

        let line_bytes = mat.bytes();
        let (display_bytes, display_len, line_number, byte_offset) =
            SinkState::prepare_line(line_bytes, mat);

        let line_content = String::from_utf8_lossy(display_bytes).into_owned();
        let mut match_byte_offsets: SmallVec<[(u32, u32); 4]> = SmallVec::new();
        let mut col = 0usize;
        let mut first = true;

        if self.case_insensitive {
            // Lowercase the display bytes into a stack buffer; positions are 1:1
            // for ASCII so no mapping is needed.
            let mut lowered = [0u8; MAX_LINE_DISPLAY_LEN];
            let len = display_bytes.len().min(MAX_LINE_DISPLAY_LEN);
            for (dst, &src) in lowered[..len].iter_mut().zip(display_bytes) {
                *dst = src.to_ascii_lowercase();
            }
            let mut start_pos = 0usize;
            while let Some(pos) = self.finder.find(&lowered[start_pos..len]) {
                let abs_start = (start_pos + pos) as u32;
                let abs_end = (abs_start + self.pattern_len).min(display_len);
                if first {
                    col = abs_start as usize;
                    first = false;
                }
                match_byte_offsets.push((abs_start, abs_end));
                start_pos += pos + 1;
            }
        } else {
            let mut start_pos = 0usize;
            while let Some(pos) = self.finder.find(&display_bytes[start_pos..]) {
                let abs_start = (start_pos + pos) as u32;
                let abs_end = (abs_start + self.pattern_len).min(display_len);
                if first {
                    col = abs_start as usize;
                    first = false;
                }
                match_byte_offsets.push((abs_start, abs_end));
                start_pos += pos + 1;
            }
        }

        self.state.push_match(
            line_number,
            col,
            byte_offset,
            line_content,
            match_byte_offsets,
        );
        Ok(true)
    }

    fn finish(&mut self, _: &Searcher, _: &grep_searcher::SinkFinish) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Sink for `Regex` mode.
///
/// Uses the compiled regex to extract precise variable-length highlight spans
/// from each matched line. No `memmem` finder is involved.
struct RegexSink<'r> {
    state: SinkState,
    re: &'r regex::bytes::Regex,
}

impl Sink for RegexSink<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        sink_match: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        if self.state.max_matches != 0 && self.state.matches.len() >= self.state.max_matches {
            return Ok(false);
        }

        let line_bytes = sink_match.bytes();
        let (display_bytes, display_len, line_number, byte_offset) =
            SinkState::prepare_line(line_bytes, sink_match);

        let line_content = String::from_utf8_lossy(display_bytes).into_owned();
        let mut match_byte_offsets: SmallVec<[(u32, u32); 4]> = SmallVec::new();
        let mut col = 0usize;
        let mut first = true;

        for m in self.re.find_iter(display_bytes) {
            let abs_start = m.start() as u32;
            let abs_end = (m.end() as u32).min(display_len);
            if first {
                col = abs_start as usize;
                first = false;
            }
            match_byte_offsets.push((abs_start, abs_end));
        }

        self.state.push_match(
            line_number,
            col,
            byte_offset,
            line_content,
            match_byte_offsets,
        );
        Ok(true)
    }

    fn finish(&mut self, _: &Searcher, _: &grep_searcher::SinkFinish) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Check if a byte is a valid UTF-8 character boundary.
#[inline]
fn is_utf8_char_boundary(b: u8) -> bool {
    // Continuation bytes have the bit pattern 10xxxxxx.
    (b as i8) >= -0x40
}

/// Build a regex from the user's grep text.
///
/// In `PlainText` mode:
/// - Escapes the input for literal matching (users type text, not regex)
/// - Applies smart case: case-insensitive unless query has uppercase
/// - Detects `\n` for multiline
///
/// In `Regex` mode:
/// - The input is passed directly to the regex engine without escaping
/// - Smart case still applies
/// - Returns `None` for invalid regex patterns — the caller falls back to literal mode
fn build_regex(pattern: &str, smart_case: bool) -> Result<regex::bytes::Regex, String> {
    if pattern.is_empty() {
        return Err("empty pattern".to_string());
    }

    let regex_pattern = if pattern.contains("\\n") {
        pattern.replace("\\n", "\n")
    } else {
        pattern.to_string()
    };

    let case_insensitive = if smart_case {
        !pattern.chars().any(|c| c.is_uppercase())
    } else {
        false
    };

    regex::bytes::RegexBuilder::new(&regex_pattern)
        .case_insensitive(case_insensitive)
        .unicode(false)
        .build()
        .map_err(|e| e.to_string())
}

/// Convert character-position indices from neo_frizbee into byte-offset
/// pairs (start, end) suitable for `match_byte_offsets`.
///
/// frizbee returns character positions (0-based index into the char
/// iterator). We need byte ranges because the UI renderer and Lua layer
/// use byte offsets for extmark highlights.
///
/// Each matched character becomes its own (byte_start, byte_end) pair.
/// Adjacent characters are merged into a single contiguous range.
fn char_indices_to_byte_offsets(line: &str, char_indices: &[usize]) -> SmallVec<[(u32, u32); 4]> {
    if char_indices.is_empty() {
        return SmallVec::new();
    }

    // Build a map: char_index -> (byte_start, byte_end) for all chars.
    // Iterating all chars is O(n) in the line length which is bounded by MAX_LINE_DISPLAY_LEN (512).
    let char_byte_ranges: Vec<(usize, usize)> = line
        .char_indices()
        .map(|(byte_pos, ch)| (byte_pos, byte_pos + ch.len_utf8()))
        .collect();

    // Convert char indices to byte ranges, merging adjacent ranges
    let mut result: SmallVec<[(u32, u32); 4]> = SmallVec::with_capacity(char_indices.len());

    for &ci in char_indices {
        if ci >= char_byte_ranges.len() {
            continue; // out of bounds (shouldn't happen with valid data)
        }
        let (start, end) = char_byte_ranges[ci];
        // Merge with previous range if adjacent
        if let Some(last) = result.last_mut()
            && last.1 == start as u32
        {
            last.1 = end as u32;
            continue;
        }
        result.push((start as u32, end as u32));
    }

    result
}

#[tracing::instrument(skip_all, level = Level::DEBUG)]
fn run_file_search<'a, F>(
    files_to_search: &[&'a FileItem],
    options: &GrepSearchOptions,
    total_files: usize,
    filtered_file_count: usize,
    regex_fallback_error: Option<String>,
    search_file: F,
) -> GrepResult<'a>
where
    F: Fn(&[u8], usize) -> Vec<GrepMatch> + Sync,
{
    let time_budget = if options.time_budget_ms > 0 {
        Some(std::time::Duration::from_millis(options.time_budget_ms))
    } else {
        None
    };

    let search_start = std::time::Instant::now();
    let page_limit = options.page_limit;

    let budget_exceeded = AtomicBool::new(false);

    // Parallel phase: search all files concurrently using rayon.
    // Every file is visited (no early-exit gaps), so per_file_results is a
    // contiguous, order-preserving subset — pagination offsets stay correct.
    // The time budget acts as the work bound; there is no separate file cap.
    let per_file_results: Vec<(usize, &'a FileItem, Vec<GrepMatch>)> = files_to_search
        .par_iter()
        .enumerate()
        .filter_map(|(idx, file)| {
            // Time budget check (relaxed — checked once per file, not per line).
            if let Some(budget) = time_budget
                && search_start.elapsed() > budget
            {
                budget_exceeded.store(true, Ordering::Relaxed);
                return None;
            }

            let mmap = file.get_mmap()?;
            let file_matches = search_file(&mmap[..], options.max_matches_per_file);

            if file_matches.is_empty() {
                return None;
            }

            Some((idx, *file, file_matches))
        })
        .collect();

    // Flatten per-file results into the final vecs in sorted order.
    // Each match stores a `file_index` pointing into `result_files` so that
    // consumers (FFI JSON, Lua) can look up file metadata without duplicating
    // it across every match from the same file.
    let mut result_files: Vec<&'a FileItem> = Vec::new();
    let mut all_matches: Vec<GrepMatch> = Vec::new();
    // files_consumed tracks how far into files_to_search we have advanced,
    // counting every file whose results were emitted (with or without matches).
    // We use the batch_idx of the last consumed file + 1, which is correct
    // because per_file_results only contains files that had matches, and
    // files between them that had no matches were still searched and can be
    // safely skipped on the next page.
    let mut files_consumed: usize = 0;

    for (batch_idx, file, file_matches) in per_file_results {
        // batch_idx is the 0-based position in files_to_search.
        // Advance files_consumed to include this file and all no-match files before it.
        files_consumed = batch_idx + 1;

        let file_result_idx = result_files.len();
        result_files.push(file);

        for mut m in file_matches {
            m.file_index = file_result_idx;
            all_matches.push(m);
        }

        // page_limit is a soft cap: we always finish the current file before
        // stopping, so no matches are dropped. A page may return up to
        // page_limit + max_matches_per_file - 1 matches in the worst case.
        if all_matches.len() >= page_limit {
            break;
        }
    }

    // If no file had any match, we searched the entire slice.
    if result_files.is_empty() {
        files_consumed = files_to_search.len();
    }

    let has_more = budget_exceeded.load(Ordering::Relaxed)
        || (all_matches.len() >= page_limit && files_consumed < files_to_search.len());

    let next_file_offset = if has_more {
        options.file_offset + files_consumed
    } else {
        0
    };

    GrepResult {
        matches: all_matches,
        files: result_files,
        total_files_searched: files_consumed,
        total_files,
        filtered_file_count,
        next_file_offset,
        regex_fallback_error,
    }
}

/// Filter files by constraints and size/binary checks, sort by frecency,
/// and apply file-based pagination.
///
/// Returns `(paginated_files, filtered_file_count)`. The paginated slice
/// is empty if the offset is past the end of available files.
fn prepare_files_to_search<'a>(
    files: &'a [FileItem],
    constraints: &[fff_query_parser::Constraint<'_>],
    options: &GrepSearchOptions,
) -> (Vec<&'a FileItem>, usize) {
    let prefiltered: Vec<&FileItem> = if constraints.is_empty() {
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

    let total_count = prefiltered.len();

    // Sort by frecency (files are stored by path, not frecency)
    let mut sorted_files = prefiltered;
    sort_with_buffer(&mut sorted_files, |a, b| {
        b.total_frecency_score
            .cmp(&a.total_frecency_score)
            .then(b.modified.cmp(&a.modified))
    });

    if options.file_offset < total_count {
        let sorted_files = sorted_files.split_off(options.file_offset);
        (sorted_files, total_count)
    } else {
        (Vec::new(), total_count)
    }
}

/// Fuzzy grep search using SIMD-accelerated `neo_frizbee::match_list`.
///
/// # Why this doesn't use `grep-searcher` / `GrepSink`
///
/// PlainText and Regex modes use the `grep-searcher` pipeline: a `Matcher`
/// finds candidate lines, and a `Sink` collects them one at a time. This
/// works well because memchr/regex can *skip* non-matching lines in O(n)
/// without scoring every one.
///
/// Fuzzy matching is fundamentally different. Every line is a candidate —
/// the Smith-Waterman score determines whether it passes, not a substring
/// or pattern test. The `Matcher::find_at` trait forces per-line calls to
/// the *reference* (scalar) smith-waterman, which is O(needle × line_len)
/// per line. For a 10k-line file that's 10k sequential reference calls.
///
/// `neo_frizbee::match_list` solves this by batching lines into
/// fixed-width SIMD buckets (4, 8, 12 … 512 bytes) and scoring 16+
/// haystacks per SIMD invocation. A single `match_list` call over the
/// entire file replaces 10k individual `match_indices` calls. We then
/// call `match_indices` *only* on the ~5-20 lines that pass `min_score`
/// to extract character highlight positions.
///
/// Line splitting uses `memchr::memchr` (the same SIMD-accelerated byte
/// search that `grep-searcher` and `bstr::ByteSlice::find_byte` use
/// internally) to locate `\n` terminators. This gives us the same
/// performance as the searcher's `LineStep` iterator without pulling in
/// the full searcher machinery.
///
/// For each file:
///   1. mmap the file, split lines via memchr '\n' (tracking line numbers + byte offsets)
///   2. Batch all lines through `match_list` (SIMD smith-waterman)
///   3. Filter results by `min_score`
///   4. Call `match_indices` only on passing lines to get character highlight offsets
fn fuzzy_grep_search<'a>(
    grep_text: &str,
    files_to_search: &[&'a FileItem],
    options: &GrepSearchOptions,
    total_files: usize,
    filtered_file_count: usize,
    case_insensitive: bool,
) -> GrepResult<'a> {
    // max_typos controls how many *needle* characters can be unmatched.
    // A transposition (e.g. "shcema" → "schema") costs ~1 typo with
    // default gap penalties. We scale max_typos by needle length:
    //   1-2 chars → 0 typos (exact subsequence only)
    //   3-5 chars → 1 typo
    //   6+  chars → 2 typos
    let max_typos = (grep_text.len() / 3).min(2);
    let frizbee_config = neo_frizbee::Config {
        prefilter: true, // SIMD prefilter rejects obvious non-matches cheaply
        max_typos: Some(max_typos as u16),
        sort: false, // We handle ordering ourselves
        scoring: neo_frizbee::Scoring {
            // Use default gap penalties. Higher values (e.g. 20) cause
            // smith-waterman to prefer *dropping needle chars* over paying
            // gap costs, which inflates the typo count and breaks
            // transposition matching ("shcema" → "schema" becomes 3 typos
            // instead of 1). Scattered matches are filtered by max_typos
            // and the match span check below instead.
            exact_match_bonus: 100,
            // gap_open_penalty: 4,
            // gap_extend_penalty: 2,
            prefix_bonus: 0,
            capitalization_bonus: if case_insensitive { 0 } else { 4 },
            ..neo_frizbee::Scoring::default()
        },
    };

    // Minimum score threshold: 50% of a perfect contiguous match.
    // With default scoring (match_score=12, matching_case_bonus=4 = 16/char),
    // a transposition costs ~5 from a gap, keeping the score well above 50%.
    let perfect_score = (grep_text.len() as u16) * 16;
    let min_score = (perfect_score * 50) / 100;

    // Maximum allowed span of matched characters in the haystack, relative
    // to needle length.
    //
    // We allow up to needle_len * 2 to accommodate fuzzy subsequence
    // matches in longer identifiers (e.g. "SortedMap" → "SortedArrayMap"
    // has span 13 for needle 9). Quality is enforced by the density and
    // gap checks below, not just span alone.
    let max_match_span = grep_text.len() * 2;
    let needle_len = grep_text.len();

    // We scale by needle_len: longer needles tolerate more gaps.
    let max_gaps = (needle_len / 4).max(1);

    run_file_search(
        files_to_search,
        options,
        total_files,
        filtered_file_count,
        None,
        |file_bytes: &[u8], max_matches_per_file: usize| {
            // Reuse grep-searcher's LineStep for SIMD-accelerated line iteration.
            // This is the same code path used by PlainText/Regex modes and is
            // verified to handle platform line endings (LF, CRLF) correctly.
            let mut stepper = LineStep::new(b'\n', 0, file_bytes.len());
            let mut file_lines: Vec<&str> = Vec::with_capacity(4096);
            let mut line_meta: Vec<(u64, u64)> = Vec::with_capacity(4096);
            let line_term_lf = grep_matcher::LineTerminator::byte(b'\n');
            let line_term_cr = grep_matcher::LineTerminator::byte(b'\r');

            let mut line_number: u64 = 1;
            while let Some(line_match) = stepper.next_match(file_bytes) {
                let byte_offset = line_match.start() as u64;

                // Strip line terminator (\n) then trailing \r using
                // grep-searcher's utility, correctly handling LF, CRLF,
                // and bare CR line endings across platforms.
                let trimmed = lines::without_terminator(
                    lines::without_terminator(&file_bytes[line_match], line_term_lf),
                    line_term_cr,
                );

                // Feed lines to match_list without truncation — truncation
                // is only needed for display, and match_list handles the
                // 512-char bucket cap internally. We only truncate lines
                // that pass scoring + post-filters below.
                //
                // Safety: files that passed `is_binary` check don't contain
                // null bytes. Source code is virtually always valid UTF-8.
                // Invalid UTF-8 lines would produce wrong match positions
                // but won't cause UB since match_indices re-validates below.
                if !trimmed.is_empty()
                    && let Ok(line_str) = std::str::from_utf8(trimmed)
                {
                    file_lines.push(line_str);
                    line_meta.push((line_number, byte_offset));
                }

                line_number += 1;
            }

            if file_lines.is_empty() {
                return Vec::new();
            }

            let matches = neo_frizbee::match_list(grep_text, &file_lines, &frizbee_config);
            let mut file_matches: Vec<GrepMatch> = Vec::new();

            for m in &matches {
                if m.score < min_score {
                    continue;
                }

                let idx = m.index as usize;
                let raw_line = file_lines[idx];

                let display_line = if raw_line.len() > MAX_LINE_DISPLAY_LEN {
                    let mut end = MAX_LINE_DISPLAY_LEN;
                    let bytes = raw_line.as_bytes();
                    // important for non ascii languages that might have character boundary at
                    // offset exactly at MAX_LINE_DISPLAY_LEN
                    while end > 0 && !is_utf8_char_boundary(bytes[end]) {
                        end -= 1;
                    }
                    &raw_line[..end]
                } else {
                    raw_line
                };

                let Some(match_indices) =
                    neo_frizbee::match_indices(grep_text, display_line, &frizbee_config)
                else {
                    continue; // something is off treat as nomatch
                };

                // Minimum matched chars: at least (needle_len - 1) characters
                // must appear in the match indices. This allows one missing
                // char (a single typo/transposition) but rejects matches that
                // only hit a partial substring (e.g. "HashMap" for "shcema").
                let min_matched = needle_len.saturating_sub(1).max(1);
                if match_indices.indices.len() < min_matched {
                    continue;
                }

                let indices = &match_indices.indices;

                if let (Some(&first), Some(&last)) = (indices.first(), indices.last()) {
                    // Span check: reject widely scattered matches.
                    let span = last - first + 1;
                    if span > max_match_span {
                        continue;
                    }

                    // Density check: matched chars / span must be dense enough.
                    // Relaxed for perfect subsequence matches (all needle chars
                    // present), stricter when typos are involved.
                    let density = (indices.len() * 100) / span;
                    let min_density = if indices.len() >= needle_len {
                        50 // Perfect subsequence — relaxed
                    } else {
                        70 // Has typos — stricter
                    };
                    if density < min_density {
                        continue;
                    }

                    // Gap count check: count discontinuities in the indices.
                    // A gap is where indices[i] != indices[i-1] + 1 (matched
                    // chars jump over unmatched haystack chars).
                    //
                    // This rejects matches where the needle chars are scattered
                    // across unrelated words in the haystack:
                    //   "struct SortedArrayMap" → 1 gap ✓
                    //   "struct SourcingProjectMetadataParts" → 6 gaps ✗
                    let gap_count = indices.windows(2).filter(|w| w[1] != w[0] + 1).count();
                    if gap_count > max_gaps {
                        continue;
                    }
                }

                let (ln, bo) = line_meta[idx];
                let match_byte_offsets =
                    char_indices_to_byte_offsets(display_line, &match_indices.indices);
                let col = match_byte_offsets
                    .first()
                    .map(|r| r.0 as usize)
                    .unwrap_or(0);

                file_matches.push(GrepMatch {
                    file_index: 0, // set by run_file_search
                    line_number: ln,
                    col,
                    byte_offset: bo,
                    line_content: display_line.to_string(),
                    match_byte_offsets,
                    fuzzy_score: Some(match_indices.score),
                });

                if max_matches_per_file != 0 && file_matches.len() >= max_matches_per_file {
                    break;
                }
            }

            file_matches
        },
    )
}

/// Perform a grep search across all indexed files.
///
/// When `query` is empty, returns git-modified/untracked files sorted by
/// frecency for the "welcome state" UI.
pub fn grep_search<'a>(
    files: &'a [FileItem],
    raw_query: &str,
    query: Option<FFFQuery<'_>>,
    options: &GrepSearchOptions,
) -> GrepResult<'a> {
    let total_files = files.len();

    // Extract the grep text and file constraints from the parsed query.
    // For grep, the search pattern is the original query with constraint tokens
    // removed. All non-constraint text tokens are collected and joined with
    // spaces to form the grep pattern:
    //   "name = *.rs someth" -> grep "name = someth" with constraint Extension("rs")
    let constraints: &[fff_query_parser::Constraint<'_>];

    let grep_text = match &query {
        Some(p) => {
            constraints = &p.constraints[..];
            p.grep_text()
        }
        None => {
            constraints = &[];
            // Single-token query (parser returned None). If the token is a
            // backslash-escaped constraint (e.g. `\*.rs`, `\/src/`, `\!test`),
            // strip the leading `\` so the literal text is searched. Other
            // backslash sequences (e.g. `\bfoo\b` in regex mode) are left alone.
            let t = raw_query.trim();
            if t.starts_with('\\') && t.len() > 1 {
                // Re-parse the unescaped suffix: if it would be a constraint,
                // the user intended an escape; strip the backslash.
                let suffix = &t[1..];
                let parser = QueryParser::new(GrepConfig);
                if parser
                    .parse(suffix)
                    .is_some_and(|q| !q.constraints.is_empty())
                {
                    suffix.to_string()
                } else {
                    t.to_string()
                }
            } else {
                t.to_string()
            }
        }
    };

    if grep_text.is_empty() {
        return GrepResult {
            total_files,
            filtered_file_count: total_files,
            next_file_offset: 0,
            matches: Vec::new(),
            files: Vec::new(),
            ..Default::default()
        };
    }

    // Filter, sort, and paginate files (shared across all modes)
    let (files_to_search, filtered_file_count) =
        prepare_files_to_search(files, constraints, options);

    if files_to_search.is_empty() {
        return GrepResult {
            total_files,
            filtered_file_count,
            next_file_offset: 0,
            ..Default::default()
        };
    }

    let case_insensitive = if options.smart_case {
        !grep_text.chars().any(|c| c.is_uppercase())
    } else {
        false
    };

    let mut regex_fallback_error: Option<String> = None;
    let regex = match options.mode {
        GrepMode::PlainText => None,
        GrepMode::Fuzzy => {
            return fuzzy_grep_search(
                &grep_text,
                &files_to_search,
                options,
                total_files,
                filtered_file_count,
                case_insensitive,
            );
        }
        GrepMode::Regex => build_regex(&grep_text, options.smart_case)
            .inspect_err(|err| {
                tracing::warn!("Regex compilation failed for {}. Error {}", grep_text, err);

                regex_fallback_error = Some(err.to_string());
            })
            .ok(),
    };

    let is_multiline = grep_text.contains("\\n");

    let effective_pattern = if is_multiline {
        grep_text.replace("\\n", "\n")
    } else {
        grep_text.to_string()
    };

    // Build the finder pattern once — used by PlainTextSink (and as a
    // literal-needle fallback anchor when regex compilation fell back to plain).
    let finder_pattern: Vec<u8> = if case_insensitive {
        effective_pattern.as_bytes().to_ascii_lowercase()
    } else {
        effective_pattern.as_bytes().to_vec()
    };
    let finder = memchr::memmem::Finder::new(&finder_pattern);
    let pattern_len = finder_pattern.len() as u32;

    // `PlainTextMatcher` is used by the grep-searcher engine for line detection.
    // `PlainTextSink` / `RegexSink` handle highlight extraction independently.
    let plain_matcher = PlainTextMatcher {
        needle: finder_pattern.clone(),
        case_insensitive,
    };

    let searcher = {
        let mut b = SearcherBuilder::new();
        b.line_number(true).multi_line(is_multiline);
        b
    }
    .build();

    // Dispatch to the appropriate sink type at the boundary — zero runtime
    // branching inside the per-line hot path.
    run_file_search(
        &files_to_search,
        options,
        total_files,
        filtered_file_count,
        regex_fallback_error,
        |file_bytes: &[u8], max_matches: usize| {
            let state = SinkState {
                file_index: 0, // set by run_file_search
                matches: Vec::new(),
                max_matches,
            };

            match regex {
                Some(ref re) => {
                    let regex_matcher = RegexMatcher {
                        regex: re,
                        is_multiline,
                    };
                    let mut sink = RegexSink { state, re };
                    if let Err(e) = searcher.search_slice(&regex_matcher, file_bytes, &mut sink) {
                        tracing::error!(error = %e, "Grep (regex) search failed");
                    }
                    sink.state.matches
                }
                None => {
                    let mut sink = PlainTextSink {
                        state,
                        finder: &finder,
                        pattern_len,
                        case_insensitive,
                    };
                    if let Err(e) = searcher.search_slice(&plain_matcher, file_bytes, &mut sink) {
                        tracing::error!(error = %e, "Grep (plain text) search failed");
                    }
                    sink.state.matches
                }
            }
        },
    )
}

/// Parse a grep query using the GrepConfig parser.
pub fn parse_grep_query(query: &str) -> Option<FFFQuery<'_>> {
    let parser = QueryParser::new(GrepConfig);
    parser.parse(query)
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_fuzzy_typo_scoring() {
        // Mirror the config from fuzzy_grep_search
        let needle = "schema";
        let max_typos = (needle.len() / 3).min(2); // 2
        let config = neo_frizbee::Config {
            prefilter: false,
            max_typos: Some(max_typos as u16),
            sort: false,
            scoring: neo_frizbee::Scoring {
                exact_match_bonus: 100,
                ..neo_frizbee::Scoring::default()
            },
        };
        let min_matched = needle.len().saturating_sub(1).max(1); // 5
        let max_match_span = needle.len() + 4; // 10

        // Helper: check if a match would pass our post-filters
        let passes = |n: &str, h: &str| -> bool {
            let Some(mi) = neo_frizbee::match_indices(n, h, &config) else {
                return false;
            };
            if mi.indices.len() < min_matched {
                return false;
            }
            if let (Some(&first), Some(&last)) = (mi.indices.first(), mi.indices.last()) {
                let span = last - first + 1;
                if span > max_match_span {
                    return false;
                }
                let density = (mi.indices.len() * 100) / span;
                if density < 70 {
                    return false;
                }
            }
            true
        };

        // Exact match: must pass
        assert!(passes("schema", "schema"));
        // Exact in longer line: must pass
        assert!(passes("schema", "  schema: String,"));
        // In identifier: must pass
        assert!(passes("schema", "pub fn validate_schema() {}"));
        // Transposition: must pass
        assert!(passes("shcema", "schema"));
        // Partial "ema" only line: must NOT pass
        assert!(!passes("schema", "it has ema in it"));
        // Completely unrelated: must NOT pass
        assert!(!passes("schema", "hello world foo bar"));
    }
}
