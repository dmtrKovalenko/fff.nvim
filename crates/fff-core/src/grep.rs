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
use tracing::{debug, warn};

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
    pub match_byte_offsets: Vec<(u32, u32)>,
    /// Fuzzy match score from neo_frizbee (only set in Fuzzy grep mode).
    pub fuzzy_score: Option<u16>,
}

/// Result of a grep search.
#[derive(Debug, Clone, Default)]
pub struct GrepResult<'a> {
    pub matches: Vec<GrepMatch>,
    /// Deduplicated file references for the returned matches.
    pub files: Vec<&'a FileItem>,
    /// Total matches collected (may be more than `matches.len()` due to page_limit).
    pub total_match_count: usize,
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
struct FffMatcher {
    regex: regex::bytes::Regex,
    is_multiline: bool,
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

/// Unified sink that collects `GrepMatch` entries from a single file search
/// for PlainText and Regex modes.
///
/// Uses mode-specific highlight extraction:
/// - PlainText: SIMD-accelerated `memchr::memmem::Finder` for literal matching
/// - Regex: Compiled regex for variable-length match extraction
///
/// Fuzzy mode bypasses GrepSink entirely — it uses SIMD `match_list` directly.
struct GrepSink<'r> {
    file_index: usize,
    matches: Vec<GrepMatch>,
    max_matches: usize,
    /// SIMD-accelerated literal finder for match position highlighting.
    /// Used in PlainText mode; also used as fallback in Regex mode for
    /// simple patterns.
    finder: &'r memchr::memmem::Finder<'r>,
    /// Length of the search pattern in bytes (for computing match end offsets).
    /// Only accurate in PlainText mode; in Regex mode the regex_highlights
    /// field provides exact match spans.
    pattern_len: u32,
    /// When true, perform ASCII case-insensitive matching for highlights.
    case_insensitive: bool,
    /// When set (Regex mode), use this regex to find exact highlight positions
    /// within matched lines instead of the literal finder.
    regex_highlights: Option<&'r regex::bytes::Regex>,
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

        // Trim trailing newline/CR directly on bytes to avoid UTF-8 conversion
        let trimmed_len = {
            let mut len = line_bytes.len();
            while len > 0 && matches!(line_bytes[len - 1], b'\n' | b'\r') {
                len -= 1;
            }
            len
        };
        let trimmed_bytes = &line_bytes[..trimmed_len];

        // Truncate for display (floor to a char boundary)
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

        // Find all match positions within the display line.
        // Mode-specific highlight extraction:
        // - Regex: use compiled regex for variable-length spans
        // - PlainText: use faster memchr::memmem literal finder
        let mut match_byte_offsets = Vec::new();
        let mut col = 0usize;
        let mut first = true;

        let line_content = String::from_utf8_lossy(display_bytes).into_owned();

        if let Some(re) = self.regex_highlights {
            // Regex mode: find all non-overlapping matches using the regex engine
            for m in re.find_iter(display_bytes) {
                let abs_start = m.start() as u32;
                let abs_end = (m.end() as u32).min(display_len);
                if first {
                    col = abs_start as usize;
                    first = false;
                }
                match_byte_offsets.push((abs_start, abs_end));
            }
        } else if self.case_insensitive {
            // PlainText case-insensitive: lowercase the display bytes on the stack,
            // find positions in the lowered copy, map back (positions are 1:1 for ASCII).
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
                start_pos += pos + 1; // advance past match start to find overlapping matches
            }
        } else {
            // PlainText case-sensitive: use memchr::memmem directly
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

        self.matches.push(GrepMatch {
            file_index: self.file_index,
            line_number,
            col,
            byte_offset,
            line_content,
            match_byte_offsets,
            fuzzy_score: None, // GrepSink is only used for PlainText/Regex modes
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
fn build_regex(
    pattern: &str,
    smart_case: bool,
    mode: GrepMode,
) -> Result<regex::bytes::Regex, String> {
    if pattern.is_empty() {
        return Err("empty pattern".to_string());
    }

    // Check for multiline: user typed literal \n
    let (effective_pattern, _is_multiline) = if pattern.contains("\\n") {
        (pattern.replace("\\n", "\n"), true)
    } else {
        (pattern.to_string(), false)
    };

    // Build the regex pattern based on mode
    let regex_pattern = match mode {
        GrepMode::PlainText => regex::escape(&effective_pattern),
        GrepMode::Regex => effective_pattern,
        GrepMode::Fuzzy => {
            // Fuzzy mode doesn't use regex at all, so this function should never be called
            return Err("build_regex called for Fuzzy mode".to_string());
        }
    };

    // Smart case: case-insensitive unless query contains uppercase
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
/// neo_frizbee returns character positions (0-based index into the char
/// iterator). We need byte ranges because the UI renderer and Lua layer
/// use byte offsets for extmark highlights.
///
/// Each matched character becomes its own (byte_start, byte_end) pair.
/// Adjacent characters are merged into a single contiguous range.
fn char_indices_to_byte_offsets(line: &str, char_indices: &[usize]) -> Vec<(u32, u32)> {
    if char_indices.is_empty() {
        return Vec::new();
    }

    // Build a map: char_index -> (byte_start, byte_end) for all chars.
    // Iterating all chars is O(n) in the line length which is bounded by MAX_LINE_DISPLAY_LEN (512).
    let char_byte_ranges: Vec<(usize, usize)> = line
        .char_indices()
        .map(|(byte_pos, ch)| (byte_pos, byte_pos + ch.len_utf8()))
        .collect();

    // Convert char indices to byte ranges, merging adjacent ranges
    let mut result: Vec<(u32, u32)> = Vec::with_capacity(char_indices.len());

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

fn run_file_search<'a, F>(
    files_to_search: &[&'a FileItem],
    options: &GrepSearchOptions,
    total_files: usize,
    filtered_file_count: usize,
    regex_fallback_error: Option<String>,
    // When true, only break on time budget if we already have enough
    // results to fill half a page (fuzzy mode collects from many files).
    require_partial_fill_for_budget_break: bool,
    mut search_file: F,
) -> GrepResult<'a>
where
    F: FnMut(&[u8], usize) -> Vec<GrepMatch>,
{
    let time_budget = if options.time_budget_ms > 0 {
        Some(std::time::Duration::from_millis(options.time_budget_ms))
    } else {
        None
    };

    let search_start = std::time::Instant::now();
    let mut total_match_count = 0usize;
    let mut files_searched_in_call = 0usize;
    let mut result_files: Vec<&'a FileItem> = Vec::new();
    let mut all_matches: Vec<GrepMatch> = Vec::new();

    for (file_idx, file) in files_to_search.iter().enumerate() {
        // Check time budget
        if let Some(budget) = time_budget
            && search_start.elapsed() > budget
        {
            if !require_partial_fill_for_budget_break || all_matches.len() >= options.page_limit / 2
            {
                break;
            }
        }

        let Some(mmap) = file.get_mmap() else {
            files_searched_in_call = file_idx + 1;
            continue;
        };

        let file_matches = search_file(&mmap[..], options.max_matches_per_file);

        files_searched_in_call = file_idx + 1;

        if !file_matches.is_empty() {
            let deduped_file_idx = result_files.len();
            result_files.push(file);

            for mut m in file_matches {
                m.file_index = deduped_file_idx;
                all_matches.push(m);
            }

            total_match_count = all_matches.len();

            if total_match_count >= options.page_limit {
                all_matches.truncate(options.page_limit);
                break;
            }
        }
    }

    let next_file_offset = if files_searched_in_call < files_to_search.len() {
        options.file_offset + files_searched_in_call
    } else {
        0 // No more files — signal end of results
    };

    GrepResult {
        matches: all_matches,
        files: result_files,
        total_match_count,
        total_files_searched: files_searched_in_call,
        total_files,
        filtered_file_count,
        next_file_offset,
        regex_fallback_error,
    }
}

/// Filter files by constraints and size/binary checks, sort by frecency,
/// and apply file-based pagination.
///
/// Returns `(paginated_files, filtered_file_count)`. The paginated list
/// is empty if the offset is past the end of available files.
fn prepare_files_to_search<'a>(
    files: &'a [FileItem],
    constraints: &[fff_query_parser::Constraint<'_>],
    options: &GrepSearchOptions,
) -> (Vec<&'a FileItem>, usize) {
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

    let filtered_file_count = sorted_files.len();

    // File-based pagination: skip to the requested file offset
    if options.file_offset < sorted_files.len() {
        let paginated: Vec<&'a FileItem> = sorted_files[options.file_offset..]
            .iter()
            .copied()
            .collect();
        (paginated, filtered_file_count)
    } else {
        (Vec::new(), filtered_file_count)
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

    // Maximum allowed gaps (discontinuities) in the match indices.
    // A gap occurs where indices[i] != indices[i-1] + 1 — meaning the
    // matched chars jump over unmatched haystack chars.
    //
    // Good matches have few gaps:
    //   "struct SortedArrayMap" for "struct SortedMap": 1 gap ("Array" skip)
    //   "schema" for "shcema": 1 gap (transposition)
    //
    // Garbage matches have many gaps:
    //   "struct SourcingProjectMetadataParts" for "struct SortedMap": 6 gaps
    //
    // We scale by needle_len: longer needles tolerate more gaps.
    //   1-3  chars → 1 gap
    //   4-7  chars → 2 gaps
    //   8-11 chars → 3 gaps
    //   12+  chars → 4 gaps, etc.
    let max_gaps = (needle_len / 4).max(1);

    run_file_search(
        files_to_search,
        options,
        total_files,
        filtered_file_count,
        None,
        true, // fuzzy: only break on budget if we have partial results
        |file_bytes: &[u8], max_matches_per_file: usize| {
            // --- Phase 1: Line splitting ---
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

                // Truncate for display ONLY for lines that passed scoring.
                let display_line = if raw_line.len() > MAX_LINE_DISPLAY_LEN {
                    // Floor to a char boundary
                    let mut end = MAX_LINE_DISPLAY_LEN;
                    let bytes = raw_line.as_bytes();
                    while end > 0 && !is_utf8_char_boundary(bytes[end]) {
                        end -= 1;
                    }
                    &raw_line[..end]
                } else {
                    raw_line
                };

                // Get character highlight positions via reference smith-waterman.
                // Called only on lines that passed SIMD score + min_score.
                let Some(mi) = neo_frizbee::match_indices(grep_text, display_line, &frizbee_config)
                else {
                    // match_indices returned None (filtered by max_typos) — skip
                    continue;
                };

                // Minimum matched chars: at least (needle_len - 1) characters
                // must appear in the match indices. This allows one missing
                // char (a single typo/transposition) but rejects matches that
                // only hit a partial substring (e.g. "HashMap" for "shcema").
                let min_matched = needle_len.saturating_sub(1).max(1);
                if mi.indices.len() < min_matched {
                    continue;
                }

                let indices = &mi.indices;

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
                let match_byte_offsets = char_indices_to_byte_offsets(display_line, &mi.indices);
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
                    fuzzy_score: Some(mi.score),
                });

                if file_matches.len() >= max_matches_per_file {
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
    query: &str,
    parsed: Option<FFFQuery<'_>>,
    options: &GrepSearchOptions,
) -> GrepResult<'a> {
    let total_files = files.len();

    // Extract the grep text and file constraints from the parsed query.
    // For grep, the search pattern is the original query with constraint tokens
    // removed. All non-constraint text tokens are collected and joined with
    // spaces to form the grep pattern:
    //   "name = *.rs someth" -> grep "name = someth" with constraint Extension("rs")
    let constraints: &[fff_query_parser::Constraint<'_>];
    let extracted_grep_text: String;

    let grep_text: &str = match &parsed {
        Some(p) => {
            constraints = &p.constraints[..];
            if constraints.is_empty() {
                // No constraints at all — the entire query is the grep pattern.
                // Still need to strip backslash escapes from tokens.
                extracted_grep_text = strip_backslash_escapes(query.trim());
                &extracted_grep_text
            } else {
                // Has constraints — rebuild grep text from the original query
                // by collecting all non-constraint tokens.
                extracted_grep_text = extract_text_from_query(query);
                &extracted_grep_text
            }
        }
        None => {
            constraints = &[];
            // Single token or simple query — strip backslash escapes
            extracted_grep_text = strip_backslash_escapes(query.trim());
            &extracted_grep_text
        }
    };

    // Empty query: return git-modified files sorted by frecency
    if grep_text.is_empty() {
        return build_empty_query_result(files, constraints, total_files);
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

    // fuzzy mode can not use sink pattern to make it more efficient and batch match 
    // all files lines using simd
    if options.mode == GrepMode::Fuzzy {
        return fuzzy_grep_search(
            grep_text,
            &files_to_search,
            options,
            total_files,
            filtered_file_count,
        );
    }

    // Build regex from the grep text (PlainText/Regex modes only).
    // On regex compilation failure in Regex mode, fall back to literal (escaped) matching
    // so the user still gets results. The error is recorded for the UI to display.
    let mut regex_fallback_error: Option<String> = None;
    let regex = match build_regex(grep_text, options.smart_case, options.mode) {
        Ok(r) => r,
        Err(err) if options.mode == GrepMode::Regex => {
            // Regex compilation failed — fall back to PlainText (escaped) mode
            warn!(
                "Regex compilation failed for {:?}, falling back to literal search: {}",
                grep_text, err
            );
            regex_fallback_error = Some(err);
            match build_regex(grep_text, options.smart_case, GrepMode::PlainText) {
                Ok(r) => r,
                Err(_) => {
                    return GrepResult {
                        total_files,
                        ..Default::default()
                    };
                }
            }
        }
        Err(_) => {
            return GrepResult {
                total_files,
                ..Default::default()
            };
        }
    };

    // Determine if multiline mode is needed
    let is_multiline = grep_text.contains("\\n");

    // Build a memchr literal finder for SIMD-accelerated highlight matching
    // in the sink. For case-insensitive (smart_case with no uppercase),
    // we search a lowered copy of each display line.
    let case_insensitive = if options.smart_case {
        !grep_text.chars().any(|c| c.is_uppercase())
    } else {
        false
    };

    // For multiline patterns, replace escaped \n with actual newline
    let effective_pattern = if is_multiline {
        grep_text.replace("\\n", "\n")
    } else {
        grep_text.to_string()
    };

    // In regex mode, the sink highlight finder uses the original literal text
    // for highlighting (we cannot use the regex pattern as a literal needle).
    // For simple regex patterns this works well; for complex patterns with
    // alternation or quantifiers, highlights may be incomplete — this is
    // acceptable since exact highlight positions are cosmetic.
    let finder_pattern: Vec<u8> = if case_insensitive {
        effective_pattern.as_bytes().to_ascii_lowercase()
    } else {
        effective_pattern.as_bytes().to_vec()
    };
    let finder = memchr::memmem::Finder::new(&finder_pattern);
    let pattern_len = finder_pattern.len() as u32;

    // In regex mode, we also keep the compiled regex for precise per-line
    // highlight extraction in the sink (variable-length matches).
    let sink_regex = if options.mode == GrepMode::Regex {
        Some(regex.clone())
    } else {
        None
    };

    let mode = options.mode;

    // Create matchers ONCE outside the parallel loop to avoid per-file allocation.
    // PlainText: avoids cloning the needle Vec for every file.
    // Regex: avoids cloning the compiled regex DFA for every file.
    let plain_matcher = PlainTextMatcher {
        needle: if case_insensitive {
            effective_pattern.as_bytes().to_ascii_lowercase()
        } else {
            effective_pattern.as_bytes().to_vec()
        },
        case_insensitive,
    };
    let regex_matcher = FffMatcher {
        regex,
        is_multiline,
    };

    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .multi_line(is_multiline)
        .build();

    debug!(
        grep_text,
        filtered_files = files_to_search.len(),
        file_offset = options.file_offset,
        page_limit = options.page_limit,
        is_multiline,
        time_budget_ms = options.time_budget_ms,
        "Starting grep search"
    );

    // Sequential search in frecency order.
    // Files are pre-sorted by frecency so the most relevant results come first.
    // Sequential iteration gives us trivial, correct pagination: we stop at
    // file K and resume from K on the next page. No atomics, no race conditions,
    // no duplicate results across pages.
    //
    // The time_budget prevents pathological queries from blocking the UI.
    // For common patterns the page_limit (typically 50) fills from the first
    // handful of high-frecency files, so sequential is fast in practice.
    run_file_search(
        &files_to_search,
        options,
        total_files,
        filtered_file_count,
        regex_fallback_error,
        false, // PlainText/Regex: break immediately on time budget
        |file_bytes: &[u8], max_matches: usize| {
            let mut sink = GrepSink {
                file_index: 0, // set by run_file_search
                matches: Vec::new(),
                max_matches,
                finder: &finder,
                pattern_len,
                case_insensitive,
                regex_highlights: sink_regex.as_ref(),
            };

            match mode {
                GrepMode::PlainText => {
                    if let Err(e) = searcher.search_slice(&plain_matcher, file_bytes, &mut sink) {
                        tracing::error!(error = %e, "Grep (plain text) search failed");
                    }
                }
                GrepMode::Regex => {
                    if let Err(e) = searcher.search_slice(&regex_matcher, file_bytes, &mut sink) {
                        tracing::error!(error = %e, "Grep (regex) search failed");
                    }
                }
                GrepMode::Fuzzy => unreachable!(),
            }

            sink.matches
        },
    )
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
            fuzzy_score: None,
        })
        .collect();

    GrepResult {
        matches,
        files: changed_files,
        total_match_count: total_matched,
        total_files_searched: 0,
        total_files,
        filtered_file_count: 0,
        next_file_offset: 0,
        regex_fallback_error: None,
    }
}

/// Extract the first consecutive run of non-constraint text tokens from a query.
///
/// Strip leading backslash from escaped tokens in a grep query string.
///
/// A token is considered "escaped" only when:
/// 1. It starts with `\`
/// 2. The remainder (after `\`) would be recognised as a constraint token
///
/// This ensures regex syntax like `\bfoo\b` and `\$100` is left untouched,
/// while `\*.rs` (escape of extension filter) and `\/src/` (escape of path
/// segment) are properly unescaped.
///
/// Returns the original string unchanged if no tokens need stripping
/// (fast path — no allocation).
fn strip_backslash_escapes(text: &str) -> String {
    // Fast path: no backslash anywhere → return as-is
    if !text.contains('\\') {
        return text.to_string();
    }

    let mut parts: Vec<&str> = Vec::new();
    let mut needs_strip = false;

    for token in text.split_whitespace() {
        if token.starts_with('\\') && token.len() > 1 && is_constraint_token(&token[1..]) {
            parts.push(&token[1..]);
            needs_strip = true;
        } else {
            parts.push(token);
        }
    }

    if needs_strip {
        parts.join(" ")
    } else {
        text.to_string()
    }
}

/// Extracts all non-constraint text tokens from a query, skipping constraint
/// tokens and joining the remaining text with spaces.
///
/// Given `"name = *.rs someth"`, this returns `"name = someth"`:
/// - `"name"` → text → collect
/// - `"="` → text → collect
/// - `"*.rs"` → constraint → skip
/// - `"someth"` → text → collect
///
/// Backslash-escaped tokens (e.g. `\*.rs`) are treated as text with the
/// leading `\` stripped.
fn extract_text_from_query(query: &str) -> String {
    let trimmed = query.trim();
    let mut parts: Vec<&str> = Vec::new();

    for token in trimmed.split_whitespace() {
        // Backslash-escaped constraint tokens are always text, never constraints
        let is_escaped =
            token.starts_with('\\') && token.len() > 1 && is_constraint_token(&token[1..]);
        let is_constraint = !is_escaped && is_constraint_token(token);

        if is_constraint {
            continue;
        }

        if is_escaped {
            // Strip the leading backslash — the user wants the literal text
            parts.push(&token[1..]);
        } else {
            parts.push(token);
        }
    }

    parts.join(" ")
}

/// Quick check if a token looks like a grep constraint.
/// This mirrors the constraint patterns recognized by GrepConfig in the query parser.
#[inline]
fn is_constraint_token(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }

    // Backslash-escaped tokens are never constraints
    if token.starts_with('\\') && token.len() > 1 {
        return false;
    }

    let bytes = token.as_bytes();
    match bytes[0] {
        // *.rs, *.toml — extension/glob constraints
        b'*' if token.len() > 2 && bytes[1] == b'.' => true,
        // /src/, /lib — path segment constraints
        b'/' if token.len() > 1 => true,
        // !test, !*.rs, !/src/ — negation constraints (any !word is a constraint)
        b'!' if token.len() > 1 => true,
        _ => {
            // Trailing slash: www/ — path segment
            if token.len() > 1 && bytes[bytes.len() - 1] == b'/' {
                return true;
            }
            // key:value — type:rust, status:modified, etc.
            if let Some(colon_pos) = token.find(':') {
                let key = &token[..colon_pos];
                if matches!(key, "type" | "status" | "st" | "g" | "git") {
                    return true;
                }
            }
            // Grep-specific glob: only path-oriented globs (contains / or {})
            // This mirrors GrepConfig::is_glob_pattern
            if zlob::has_wildcards(token, zlob::ZlobFlags::RECOMMENDED) {
                if bytes.contains(&b'/') {
                    return true;
                }
                if bytes.contains(&b'{') && bytes.contains(&b'}') {
                    return true;
                }
            }
            false
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
    fn test_extract_text_from_query_simple() {
        assert_eq!(extract_text_from_query("name"), "name");
    }

    #[test]
    fn test_extract_text_from_query_with_spaces() {
        assert_eq!(extract_text_from_query("name ="), "name =");
    }

    #[test]
    fn test_extract_text_from_query_with_constraint_after() {
        assert_eq!(extract_text_from_query("name = *.rs"), "name =");
    }

    #[test]
    fn test_extract_text_from_query_with_constraint_between_text() {
        assert_eq!(
            extract_text_from_query("name = *.rs someth"),
            "name = someth"
        );
    }

    #[test]
    fn test_extract_text_from_query_leading_constraint() {
        assert_eq!(extract_text_from_query("*.rs name ="), "name =");
    }

    #[test]
    fn test_extract_text_from_query_only_constraints() {
        assert_eq!(extract_text_from_query("*.rs /src/"), "");
    }

    #[test]
    fn test_extract_text_from_query_empty() {
        assert_eq!(extract_text_from_query(""), "");
    }

    #[test]
    fn test_extract_text_from_query_path_constraint() {
        assert_eq!(extract_text_from_query("name /src/ value"), "name value");
    }

    #[test]
    fn test_extract_text_from_query_negation() {
        assert_eq!(extract_text_from_query("name !*.rs value"), "name value");
    }

    #[test]
    fn test_is_constraint_token() {
        assert!(is_constraint_token("*.rs"));
        assert!(is_constraint_token("/src/"));
        assert!(is_constraint_token("/lib"));
        assert!(is_constraint_token("www/"));
        assert!(is_constraint_token("!*.rs"));
        assert!(is_constraint_token("!/src/"));
        assert!(is_constraint_token("!test")); // negated text is also a constraint
        assert!(is_constraint_token("type:rust"));
        assert!(is_constraint_token("status:modified"));

        assert!(!is_constraint_token("name"));
        assert!(!is_constraint_token("="));
        assert!(!is_constraint_token("fn"));
        assert!(!is_constraint_token("hello:world")); // unknown key
    }

    #[test]
    fn test_is_constraint_token_backslash_escape() {
        // Backslash-escaped tokens are never constraints
        assert!(!is_constraint_token("\\*.rs"));
        assert!(!is_constraint_token("\\/src/"));
        assert!(!is_constraint_token("\\!*.rs"));
        assert!(!is_constraint_token("\\type:rust"));
    }

    #[test]
    fn test_is_constraint_token_grep_globs() {
        // Path-oriented globs ARE constraints
        assert!(is_constraint_token("src/**/*.rs"));
        assert!(is_constraint_token("*/tests/*"));
        // Brace expansion IS a constraint
        assert!(is_constraint_token("{src,lib}"));
        // Bare wildcards without / or {} are NOT constraints
        assert!(!is_constraint_token("foo?"));
        assert!(!is_constraint_token("arr[0]"));
        assert!(!is_constraint_token("a*b"));
    }

    #[test]
    fn test_extract_text_from_query_backslash_escape() {
        // Escaped extension should be text with \ stripped
        assert_eq!(extract_text_from_query("\\*.rs foo"), "*.rs foo");
        // Escaped path segment should be text with \ stripped
        assert_eq!(extract_text_from_query("\\/src/ foo"), "/src/ foo");
        // Escaped negation should be text with \ stripped
        assert_eq!(extract_text_from_query("\\!test foo"), "!test foo");
    }

    #[test]
    fn test_extract_text_from_query_question_mark() {
        // ? should be treated as text, not a glob
        assert_eq!(extract_text_from_query("foo?"), "foo?");
        assert_eq!(extract_text_from_query("foo? bar"), "foo? bar");
    }

    #[test]
    fn test_extract_text_from_query_bracket() {
        // [] should be treated as text, not a glob
        assert_eq!(extract_text_from_query("arr[0]"), "arr[0]");
        assert_eq!(extract_text_from_query("arr[0] more"), "arr[0] more");
    }

    #[test]
    fn test_extract_text_from_query_path_glob_is_constraint() {
        // Path glob should be skipped as a constraint
        assert_eq!(extract_text_from_query("pattern src/**/*.rs"), "pattern");
    }

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
