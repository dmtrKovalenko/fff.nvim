use crate::{
    bigram_filter::{BigramFilter, BigramOverlay, extract_bigrams},
    bigram_query::{fuzzy_to_bigram_query, regex_to_bigram_query},
    constraints::{ConstraintPlan, ConstraintsBuffers},
    simd_string_utils::memmem,
    sort_buffer::sort_with_buffer,
    types::{ContentCacheBudget, FileItem, FileSliceExt, MmapSlot},
};
use aho_corasick::AhoCorasick;
use fff_grep::{
    Searcher, SearcherBuilder, Sink, SinkMatch,
    matcher::{Match, Matcher, NoError},
};
use fff_query_parser::{Constraint, FFFQuery, GrepConfig, QueryParser};
use rayon::prelude::*;
use smallvec::SmallVec;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::Level;

use super::utils::{GrepResult, strip_line_terminators};

#[cfg(feature = "definitions")]
#[inline]
pub(super) fn classify_definition(enabled: bool, line: &str) -> bool {
    enabled && super::classify::is_definition_line(line)
}

#[cfg(not(feature = "definitions"))]
#[inline]
pub(super) fn classify_definition(_enabled: bool, _line: &str) -> bool {
    false
}

/// Check if `text` contains `\n` that is NOT preceded by another `\`.
///
/// `\n` -> true (user wants multiline search)
/// `\\n` -> false (escaped backslash followed by literal `n`, e.g. `\\nvim-data`)
#[inline]
pub(super) fn has_unescaped_newline_escape(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len().saturating_sub(1) {
        if bytes[i] == b'\\' {
            if bytes[i + 1] == b'n' {
                // Count consecutive backslashes ending at position i
                let mut backslash_count = 1;
                while backslash_count <= i && bytes[i - backslash_count] == b'\\' {
                    backslash_count += 1;
                }
                // Odd number of backslashes before 'n' -> real \n escape
                if backslash_count % 2 == 1 {
                    return true;
                }
            }
            // Skip past the escaped character
            i += 2;
        } else {
            i += 1;
        }
    }
    false
}

/// Replace only unescaped `\n` sequences with real newlines.
///
/// `\n` -> newline character
/// `\\n` -> preserved as-is (literal backslash + `n`)
pub(super) fn replace_unescaped_newline_escapes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'n' {
                let mut backslash_count = 1;
                while backslash_count <= i && bytes[i - backslash_count] == b'\\' {
                    backslash_count += 1;
                }
                if backslash_count % 2 == 1 {
                    result.push(b'\n');
                    i += 2;
                    continue;
                }
            }
            result.push(bytes[i]);
            i += 1;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(result).unwrap_or_else(|_| text.to_string())
}

/// Controls how the grep pattern is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrepMode {
    /// Literal plain text match: default path that doesn't require any regex machinery
    #[default]
    PlainText,
    /// Regex mode: uses the same exact matching engine as ripgrep
    Regex,
    /// Smart fuzzy mode, allows user to make either a couple of single char typos or long gaps
    /// e.g. shcema -> shcema, or UserController -> UserAuthController
    ///
    /// Significatnly slower than plain text, especially on unindexed FilePicker
    Fuzzy,
}

/// A single content match within a file
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
    /// Whether the matched line looks like a definition (struct, fn, class, etc.).
    /// Computed at match time so output formatters don't need to re-scan.
    pub is_definition: bool,
    /// Lines before the match (for context display). Empty when context is 0.
    pub context_before: Vec<String>,
    /// Lines after the match (for context display). Empty when context is 0.
    pub context_after: Vec<String>,
}

impl GrepMatch {
    /// Strip leading whitespace from `line_content` and all context lines,
    /// adjusting `col` and `match_byte_offsets` so highlights remain correct.
    pub fn trim_leading_whitespace(&mut self) {
        let strip_len = self.line_content.len() - self.line_content.trim_start().len();
        if strip_len > 0 {
            self.line_content.drain(..strip_len);
            let off = strip_len as u32;
            self.col = self.col.saturating_sub(strip_len);
            for range in &mut self.match_byte_offsets {
                range.0 = range.0.saturating_sub(off);
                range.1 = range.1.saturating_sub(off);
            }
        }
        for line in &mut self.context_before {
            let n = line.len() - line.trim_start().len();
            if n > 0 {
                line.drain(..n);
            }
        }
        for line in &mut self.context_after {
            let n = line.len() - line.trim_start().len();
            if n > 0 {
                line.drain(..n);
            }
        }
    }
}

pub use crate::constants::MAX_FFFILE_SIZE;

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
    /// Number of context lines to include before each match. 0 = disabled.
    pub before_context: usize,
    /// Number of context lines to include after each match. 0 = disabled.
    pub after_context: usize,
    /// Whether to classify each match as a definition line. Adds ~2% overhead
    /// on large repos; disable for interactive grep where it is not needed.
    pub classify_definitions: bool,
    /// Strip leading whitespace from matched lines and context lines, adjusting
    /// highlight byte offsets accordingly. Useful for AI/MCP consumers and UIs
    /// that don't need indentation. Default: false.
    pub trim_whitespace: bool,
    /// External abort signal. When provided, overrides the picker's internal
    /// cancellation flag. Set to `true` to stop the search early and return
    /// partial results. Omit (or use `..Default::default()`) to let the
    /// picker manage cancellation.
    pub abort_signal: Option<Arc<AtomicBool>>,
}

impl Default for GrepSearchOptions {
    fn default() -> Self {
        Self {
            max_file_size: MAX_FFFILE_SIZE,
            max_matches_per_file: 200,
            smart_case: true,
            file_offset: 0,
            page_limit: 50,
            mode: GrepMode::default(),
            time_budget_ms: 0,
            before_context: 0,
            after_context: 0,
            classify_definitions: false,
            trim_whitespace: false,
            abort_signal: None,
        }
    }
}

#[derive(Clone, Copy)]
struct GrepContext<'a, 'b> {
    total_files: usize,
    filtered_file_count: usize,
    budget: &'a ContentCacheBudget,
    base_path: &'a Path,
    arena: crate::simd_path::ArenaPtr,
    overflow_arena: crate::simd_path::ArenaPtr,
    prefilter: Option<&'a memchr::memmem::Finder<'b>>,
    prefilter_case_insensitive: bool,
    abort_signal: &'a AtomicBool,
}

impl GrepContext<'_, '_> {
    #[inline]
    fn arena_for_file(&self, file: &FileItem) -> crate::simd_path::ArenaPtr {
        if file.is_overflow() {
            self.overflow_arena
        } else {
            self.arena
        }
    }
}

struct RegexMatcher<'r> {
    regex: &'r regex::bytes::Regex,
    is_multiline: bool,
}

impl Matcher for RegexMatcher<'_> {
    type Error = NoError;

    #[inline]
    fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<Match>, NoError> {
        Ok(self
            .regex
            .find_at(haystack, at)
            .map(|m| Match::new(m.start(), m.end())))
    }

    #[inline]
    fn line_terminator(&self) -> Option<fff_grep::LineTerminator> {
        if self.is_multiline {
            None
        } else {
            Some(fff_grep::LineTerminator::byte(b'\n'))
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
struct PlainTextMatcher<'a> {
    /// Case-folded needle bytes for case-insensitive matching.
    /// When case-sensitive, this is the original pattern bytes.
    needle: &'a [u8],
    case_insensitive: bool,
}

impl Matcher for PlainTextMatcher<'_> {
    type Error = NoError;

    #[inline]
    fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<Match>, NoError> {
        let hay = &haystack[at..];

        let found = if self.case_insensitive {
            memmem::find(hay, self.needle)
        } else {
            memchr::memmem::find(hay, self.needle)
        };

        Ok(found.map(|pos| Match::new(at + pos, at + pos + self.needle.len())))
    }

    #[inline]
    fn line_terminator(&self) -> Option<fff_grep::LineTerminator> {
        Some(fff_grep::LineTerminator::byte(b'\n'))
    }
}

/// Maximum bytes of a matched line to keep for display. Prevents minified
/// JS or huge single-line files from blowing up memory.
const MAX_LINE_DISPLAY_LEN: usize = 512;

struct SinkState {
    file_index: usize,
    matches: Vec<GrepMatch>,
    max_matches: usize,
    before_context: usize,
    after_context: usize,
    classify_definitions: bool,
}

impl SinkState {
    #[inline]
    fn prepare_line<'a>(line_bytes: &'a [u8], mat: &SinkMatch<'_>) -> (&'a [u8], u32, u64, u64) {
        let line_number = mat.line_number().unwrap_or(0);
        let byte_offset = mat.absolute_byte_offset();

        // Trim trailing newline/CR directly on bytes to avoid UTF-8 conversion.
        let trimmed_bytes = strip_line_terminators(line_bytes);

        // Truncate for display (floor to a char boundary).
        let display_bytes = truncate_display_bytes(trimmed_bytes);

        let display_len = display_bytes.len() as u32;
        (display_bytes, display_len, line_number, byte_offset)
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn push_match(
        &mut self,
        line_number: u64,
        col: usize,
        byte_offset: u64,
        line_content: String,
        match_byte_offsets: SmallVec<[(u32, u32); 4]>,
        context_before: Vec<String>,
        context_after: Vec<String>,
    ) {
        let is_definition = classify_definition(self.classify_definitions, &line_content);
        self.matches.push(GrepMatch {
            file_index: self.file_index,
            line_number,
            col,
            byte_offset,
            line_content,
            match_byte_offsets,
            fuzzy_score: None,
            is_definition,
            context_before,
            context_after,
        });
    }

    /// Extract context lines from the full buffer around a matched region.
    fn extract_context(&self, mat: &SinkMatch<'_>) -> (Vec<String>, Vec<String>) {
        if self.before_context == 0 && self.after_context == 0 {
            return (Vec::new(), Vec::new());
        }

        let buffer = mat.buffer();
        let range = mat.bytes_range_in_buffer();

        let mut before = Vec::new();
        if self.before_context > 0 && range.start > 0 {
            // Walk backward from the start of the match line to find preceding lines
            let mut pos = range.start;
            let mut lines_found = 0;
            while lines_found < self.before_context && pos > 0 {
                // Skip the newline just before our current position
                pos -= 1;
                // Find the previous newline
                let line_start = match memchr::memrchr(b'\n', &buffer[..pos]) {
                    Some(nl) => nl + 1,
                    None => 0,
                };
                let line = &buffer[line_start..pos];
                // Trim trailing \r
                let line = if line.last() == Some(&b'\r') {
                    &line[..line.len() - 1]
                } else {
                    line
                };
                let truncated = truncate_display_bytes(line);
                before.push(String::from_utf8_lossy(truncated).into_owned());
                pos = line_start;
                lines_found += 1;
            }
            before.reverse();
        }

        let mut after = Vec::new();
        if self.after_context > 0 && range.end < buffer.len() {
            let mut pos = range.end;
            let mut lines_found = 0;
            while lines_found < self.after_context && pos < buffer.len() {
                // Find the next newline
                let line_end = match memchr::memchr(b'\n', &buffer[pos..]) {
                    Some(nl) => pos + nl,
                    None => buffer.len(),
                };
                let line = &buffer[pos..line_end];
                // Trim trailing \r
                let line = if line.last() == Some(&b'\r') {
                    &line[..line.len() - 1]
                } else {
                    line
                };
                let truncated = truncate_display_bytes(line);
                after.push(String::from_utf8_lossy(truncated).into_owned());
                pos = if line_end < buffer.len() {
                    line_end + 1 // skip past \n
                } else {
                    buffer.len()
                };
                lines_found += 1;
            }
        }

        (before, after)
    }
}

/// Truncate a byte slice for display, respecting UTF-8 char boundaries.
#[inline]
pub(super) fn truncate_display_bytes(bytes: &[u8]) -> &[u8] {
    if bytes.len() <= MAX_LINE_DISPLAY_LEN {
        bytes
    } else {
        let mut end = MAX_LINE_DISPLAY_LEN;
        while end > 0 && !is_utf8_char_boundary(bytes[end]) {
            end -= 1;
        }
        &bytes[..end]
    }
}

/// Sink for `PlainText` mode.
///
/// Highlights are extracted with `memchr::memmem::Finder` (case-sensitive)
/// or the SIMD `simd_string_utils::memmem` search (case-insensitive). No regex engine is
/// involved at any point.
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
            // The finder was built over the lowered pattern, so its needle is
            // exactly the `needle_lower` expected by `memmem::find`.
            let needle_lower = self.finder.needle();
            let mut start_pos = 0usize;
            while let Some(pos) = memmem::find(&display_bytes[start_pos..], needle_lower) {
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

        let (context_before, context_after) = self.state.extract_context(mat);
        self.state.push_match(
            line_number,
            col,
            byte_offset,
            line_content,
            match_byte_offsets,
            context_before,
            context_after,
        );
        Ok(true)
    }

    fn finish(&mut self, _: &Searcher, _: &fff_grep::SinkFinish) -> Result<(), Self::Error> {
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

        let (context_before, context_after) = self.state.extract_context(sink_match);
        self.state.push_match(
            line_number,
            col,
            byte_offset,
            line_content,
            match_byte_offsets,
            context_before,
            context_after,
        );
        Ok(true)
    }

    fn finish(&mut self, _: &Searcher, _: &fff_grep::SinkFinish) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// A `grep_matcher::Matcher` backed by Aho-Corasick for multi-pattern search.
///
/// Finds the first occurrence of any pattern starting at the given offset.
/// Always reports `\n` as the line terminator for the fast candidate-line path.
struct AhoCorasickMatcher<'a> {
    ac: &'a AhoCorasick,
}

impl Matcher for AhoCorasickMatcher<'_> {
    type Error = NoError;

    #[inline]
    fn find_at(&self, haystack: &[u8], at: usize) -> std::result::Result<Option<Match>, NoError> {
        let hay = &haystack[at..];
        let found: Option<aho_corasick::Match> = self.ac.find(hay);
        Ok(found.map(|m| Match::new(at + m.start(), at + m.end())))
    }

    #[inline]
    fn line_terminator(&self) -> Option<fff_grep::LineTerminator> {
        Some(fff_grep::LineTerminator::byte(b'\n'))
    }
}

/// Sink for Aho-Corasick multi-pattern mode.
///
/// Collects all pattern match positions on each matched line for highlighting.
struct AhoCorasickSink<'a> {
    state: SinkState,
    ac: &'a AhoCorasick,
}

impl Sink for AhoCorasickSink<'_> {
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

        for m in self.ac.find_iter(display_bytes as &[u8]) {
            let abs_start = m.start() as u32;
            let abs_end = (m.end() as u32).min(display_len);
            if first {
                col = abs_start as usize;
                first = false;
            }
            match_byte_offsets.push((abs_start, abs_end));
        }

        let (context_before, context_after) = self.state.extract_context(mat);
        self.state.push_match(
            line_number,
            col,
            byte_offset,
            line_content,
            match_byte_offsets,
            context_before,
            context_after,
        );
        Ok(true)
    }

    fn finish(&mut self, _: &Searcher, _: &fff_grep::SinkFinish) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Multi-pattern OR search using Aho-Corasick.
///
/// Builds a single automaton from all patterns and searches each file in one
/// pass. This is significantly faster than regex alternation for literal text
/// searches because Aho-Corasick uses SIMD-accelerated multi-needle matching.
///
/// Returns the same `GrepResult` type as `grep_search`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn multi_grep_search<'a>(
    files: &'a [FileItem],
    patterns: &[&str],
    constraints: &[fff_query_parser::Constraint<'_>],
    options: &GrepSearchOptions,
    budget: &ContentCacheBudget,
    bigram_index: Option<&BigramFilter>,
    bigram_overlay: Option<&BigramOverlay>,
    abort_signal: &AtomicBool,
    base_path: &Path,
    arena: crate::simd_path::ArenaPtr,
    overflow_arena: crate::simd_path::ArenaPtr,
) -> GrepResult<'a> {
    let total_files = files.live_count();

    if patterns.is_empty() || patterns.iter().all(|p| p.is_empty()) {
        return GrepResult::empty(total_files, total_files);
    }

    // Bigram prefiltering: OR the candidate bitsets for each pattern.
    // A file is a candidate if it matches ANY of the patterns' bigrams.
    let bigram_candidates = if let Some(idx) = bigram_index
        && idx.is_ready()
    {
        let mut combined: Option<Vec<u64>> = None;
        for pattern in patterns {
            if let Some(candidates) = idx.query(pattern.as_bytes()) {
                combined = Some(match combined {
                    None => candidates,
                    Some(mut acc) => {
                        // OR: file is candidate if it matches any pattern
                        acc.iter_mut()
                            .zip(candidates.iter())
                            .for_each(|(a, b)| *a |= *b);
                        acc
                    }
                });
            }
        }

        if let Some(ref mut candidates) = combined
            && let Some(overlay) = bigram_overlay
        {
            for pattern in patterns {
                let pattern_bigrams = extract_bigrams(pattern.as_bytes());
                for file_idx in overlay.query_modified(&pattern_bigrams) {
                    let word = file_idx / 64;
                    if word < candidates.len() {
                        candidates[word] |= 1u64 << (file_idx % 64);
                    }
                }
            }
        }

        combined
    } else {
        None
    };

    let base_file_count = match bigram_overlay {
        Some(bigram_overlay) => bigram_overlay.base_file_count(),
        None => files.len(),
    };

    let (mut files_to_search, mut filtered_file_count) = prefilter_files(
        files,
        constraints,
        bigram_candidates.as_deref(),
        base_file_count,
        options,
        arena,
        overflow_arena,
    );

    // If constraints yielded 0 files and we had FilePath constraints,
    // retry without them (the path token was likely part of the search text).
    if files_to_search.is_empty()
        && let Some(stripped) = strip_file_path_constraint_if_present(constraints)
    {
        let (retry_files, retry_count) = prefilter_files(
            files,
            &stripped,
            bigram_candidates.as_deref(),
            base_file_count,
            options,
            arena,
            overflow_arena,
        );
        files_to_search = retry_files;
        filtered_file_count = retry_count;
    }

    if files_to_search.is_empty() {
        return GrepResult::empty(total_files, filtered_file_count);
    }

    // Smart case: case-insensitive when all patterns are lowercase
    let case_insensitive = if options.smart_case {
        !patterns.iter().any(|p| p.chars().any(|c| c.is_uppercase()))
    } else {
        false
    };

    let ac = aho_corasick::AhoCorasickBuilder::new()
        .ascii_case_insensitive(case_insensitive)
        .build(patterns)
        .expect("Aho-Corasick build should not fail for literal patterns");

    let searcher = {
        let mut b = SearcherBuilder::new();
        b.line_number(true);
        b
    }
    .build();

    let ac_matcher = AhoCorasickMatcher { ac: &ac };
    perform_grep(
        &files_to_search,
        options,
        &GrepContext {
            total_files,
            filtered_file_count,
            budget,
            base_path,
            arena,
            overflow_arena,
            prefilter: None, // no memmem prefilter for multi-pattern search
            prefilter_case_insensitive: false,
            abort_signal,
        },
        |file_bytes: &[u8], max_matches: usize| {
            let state = SinkState {
                file_index: 0,
                matches: Vec::with_capacity(4),
                max_matches,
                before_context: options.before_context,
                after_context: options.after_context,
                classify_definitions: options.classify_definitions,
            };

            let mut sink = AhoCorasickSink { state, ac: &ac };

            if let Err(e) = searcher.search_slice(&ac_matcher, file_bytes, &mut sink) {
                tracing::error!(error = %e, "Grep (aho-corasick multi) search failed");
            }

            sink.state.matches
        },
    )
}

// copied from the rust u8 private method
#[inline]
const fn is_utf8_char_boundary(b: u8) -> bool {
    (b as i8) >= -0x40
}

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
        .multi_line(true)
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
pub(super) fn char_indices_to_byte_offsets(
    line: &str,
    char_indices: &[usize],
) -> SmallVec<[(u32, u32); 4]> {
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

#[tracing::instrument(
    skip_all,
    level = Level::DEBUG,
    fields(prefiltered_count = files_to_search.len())
)]
fn perform_grep<'a, F>(
    files_to_search: &[&'a FileItem],
    options: &GrepSearchOptions,
    ctx: &GrepContext<'_, '_>,
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

    let mut result_files: Vec<&'a FileItem> = Vec::new();
    let mut all_matches: Vec<GrepMatch> = Vec::new();
    let mut files_consumed: usize = 0;
    let mut page_filled = false;

    // Each chunk is a rayon barrier. A flat small chunk over 500k files = ~7800
    // barriers; x2 growth makes it logarithmic. But a too-aggressive growth
    // over-scans: when a page fills mid-chunk, the whole submitted chunk still
    // runs.
    //
    // So only grow when the prefilter is weak (large candidate set);
    // when bigram cut the set in half, keep fixed small chunks for cheap page-fill termination.
    let base_chunk = rayon::current_num_threads() * 4;
    let prefilter_strong = ctx.total_files > 0 && files_to_search.len() * 2 < ctx.total_files;
    let max_chunk = if prefilter_strong {
        base_chunk
    } else {
        (base_chunk * 256).max(8 * 1024)
    };
    let growth = if prefilter_strong { 1 } else { 2 };
    let mut chunk_size = base_chunk;
    let mut chunk_start = 0;

    while chunk_start < files_to_search.len() {
        let chunk_end = (chunk_start + chunk_size).min(files_to_search.len());
        let chunk = &files_to_search[chunk_start..chunk_end];
        chunk_start = chunk_end;
        chunk_size = (chunk_size * growth).min(max_chunk);
        let chunk_offset = files_consumed;

        let chunk_results: Vec<(usize, &'a FileItem, Vec<GrepMatch>)> = chunk
            .par_iter()
            .enumerate()
            .map_init(
                // tested it out a few times, this is just fine for rayon worker in this specific
                // case it doesn't reallocate this many times and it is actually faster than using
                // scoped threads with a predefined local scratch buffers because of spawn cost
                || (Vec::with_capacity(64 * 1024), MmapSlot::default()),
                |(buf, mmap_slot), (local_idx, file)| {
                    // perform all the atomic machinery on every 8th
                    if local_idx % 8 == 0 {
                        let mut need_abort = ctx.abort_signal.load(Ordering::Relaxed);
                        if !need_abort
                            && let Some(budget) = time_budget
                            && all_matches.len() > 1
                            && search_start.elapsed() > budget
                        {
                            need_abort = true;
                        }

                        if need_abort {
                            budget_exceeded.store(true, Ordering::Relaxed);
                            return None;
                        }
                    }

                    let content = file.get_content_for_search(
                        buf,
                        mmap_slot,
                        ctx.arena_for_file(file),
                        ctx.base_path,
                        ctx.budget,
                    )?;

                    // Fast whole-file memmem check before entering the
                    // grep-searcher machinery. Skips Vec alloc, Searcher
                    // setup, and line-splitting for files that can't match.
                    if let Some(pf) = ctx.prefilter {
                        let found = if ctx.prefilter_case_insensitive {
                            memmem::find(content, pf.needle()).is_some()
                        } else {
                            pf.find(content).is_some()
                        };
                        if !found {
                            return None;
                        }
                    }

                    let file_matches = search_file(content, options.max_matches_per_file);

                    if file_matches.is_empty() {
                        return None;
                    }

                    Some((chunk_offset + local_idx, *file, file_matches))
                },
            )
            .flatten()
            .collect();

        // Every file in the chunk was visited by rayon (matched or not).
        files_consumed = chunk_offset + chunk.len();

        // Flatten this chunk's results into the accumulator.
        for (batch_idx, file, file_matches) in chunk_results {
            let file_result_idx = result_files.len();
            result_files.push(file);

            for mut m in file_matches {
                m.file_index = file_result_idx;
                if options.trim_whitespace {
                    m.trim_leading_whitespace();
                }
                all_matches.push(m);
            }

            if all_matches.len() >= page_limit {
                // Tighten files_consumed to the file that tipped us over so
                // the next page resumes right after it.
                files_consumed = batch_idx + 1;
                page_filled = true;
                break;
            }
        }

        if page_filled || budget_exceeded.load(Ordering::Relaxed) {
            break;
        }
    }

    // If no file had any match, we searched the entire slice.
    if result_files.is_empty() {
        files_consumed = files_to_search.len();
    }

    let has_more = budget_exceeded.load(Ordering::Relaxed)
        || (page_filled && files_consumed < files_to_search.len());

    let next_file_offset = if has_more {
        options.file_offset + files_consumed
    } else {
        0
    };

    GrepResult {
        matches: all_matches,
        files_with_matches: result_files.len(),
        files: result_files,
        total_files_searched: files_consumed,
        total_files: ctx.total_files,
        filtered_file_count: ctx.filtered_file_count,
        next_file_offset,
        regex_fallback_error: None,
    }
}

/// Single pass prefilter that doesn't involve file reading
/// allocates only amount of memory required for storing references of the FileItems have to be
/// opened for grepping unaviodably, in the worst case allocates N * <word> memory if no prefilter needed
fn prefilter_files<'a>(
    files: &'a [FileItem],
    constraints: &[fff_query_parser::Constraint<'_>],
    bigram_candidates: Option<&[u64]>,
    base_count: usize,
    options: &GrepSearchOptions,
    arena: crate::simd_path::ArenaPtr,
    overflow_arena: crate::simd_path::ArenaPtr,
) -> (Vec<&'a FileItem>, usize) {
    let max_file_size = options.max_file_size;
    let plan = if constraints.is_empty() {
        None
    } else {
        Some(ConstraintPlan::build(
            constraints,
            files,
            arena,
            overflow_arena,
        ))
    };

    let mut scratch = ConstraintsBuffers::new();

    #[inline(always)]
    fn basic_prefilter(file: &FileItem, max: u64) -> bool {
        !file.is_deleted() && !file.is_binary() && file.size > 0 && file.size <= max
    }

    // squeeze as much prefilters into a single loop as possible
    let mut prefiltered: Vec<&FileItem> = match bigram_candidates {
        Some(candidates) => {
            let boundary = base_count.min(files.len());
            let (indexed, tail) = files.split_at(boundary);

            let cap = BigramFilter::count_candidates(candidates) + tail.len();
            let mut out: Vec<&FileItem> = Vec::with_capacity(cap);

            let full_words = boundary / 64;
            let last_word_bits = boundary % 64;

            // we need this because we already had a regression of the wrong bit
            // has been set for the very last word based on the overlay, it's pretty cheap
            macro_rules! evaluate_bigram_match_word {
                ($word:expr, $base:expr) => {{
                    let mut bits: u64 = $word;
                    while bits != 0 {
                        let bit = bits.trailing_zeros() as usize;
                        let file_idx = $base + bit;
                        bits &= bits - 1;

                        let f = unsafe { indexed.get_unchecked(file_idx) };
                        if !basic_prefilter(f, max_file_size) {
                            continue;
                        }
                        if let Some(plan) = plan.as_ref()
                            && !plan.matches(f, file_idx, arena, overflow_arena, &mut scratch)
                        {
                            continue;
                        }
                        out.push(f);
                    }
                }};
            }

            // Full words: every set bit guaranteed `< boundary`.
            for (word_idx, &word) in candidates.iter().take(full_words).enumerate() {
                if word != 0 {
                    evaluate_bigram_match_word!(word, word_idx * 64);
                }
            }

            // Last partial word: mask bits past `boundary` once at word load.
            if last_word_bits != 0 {
                // this will get only (mod 64) bits from the last word guaratee that it's 0 padded
                let last_mask: u64 = (1u64 << last_word_bits) - 1;
                let word = candidates[full_words] & last_mask;
                if word != 0 {
                    evaluate_bigram_match_word!(word, full_words * 64);
                }
            }

            // Sequential processing for non-bigrammable files: they are always in the end
            for (offset, f) in tail.iter().enumerate() {
                if !basic_prefilter(f, max_file_size) {
                    continue;
                }
                if let Some(ref p) = plan
                    && !p.matches(f, boundary + offset, arena, overflow_arena, &mut scratch)
                {
                    continue;
                }
                out.push(f);
            }

            out
        }
        // this will be executed if there is no bigram, in the worst case it will allocate
        // whole array of files but probability in the real repo of NO preflter working is so
        // low that we just ignore that, usually there would be at least a few files excluded
        None => {
            let mut out: Vec<&FileItem> = Vec::new();
            for (idx, f) in files.iter().enumerate() {
                if !basic_prefilter(f, max_file_size) {
                    continue;
                }
                if let Some(ref p) = plan
                    && !p.matches(f, idx, arena, overflow_arena, &mut scratch)
                {
                    continue;
                }
                out.push(f);
            }
            out
        }
    };

    let total_count = prefiltered.len();

    sort_with_buffer(&mut prefiltered, |a, b| {
        b.total_frecency_score()
            .cmp(&a.total_frecency_score())
            .then(b.modified.cmp(&a.modified))
    });

    if options.file_offset > 0 && options.file_offset < total_count {
        let paginated = prefiltered.split_off(options.file_offset);
        (paginated, total_count)
    } else if options.file_offset >= total_count {
        (Vec::new(), total_count)
    } else {
        (prefiltered, total_count)
    }
}

/// Perform a grep search across all indexed files.
///
/// When `query` is empty, returns git-modified/untracked files sorted by
/// frecency for the "welcome state" UI.
#[tracing::instrument(skip_all, fields(file_count = files.len()))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn grep_search<'a>(
    files: &'a [FileItem],
    query: &FFFQuery<'_>,
    options: &GrepSearchOptions,
    budget: &ContentCacheBudget,
    bigram_index: Option<&BigramFilter>,
    bigram_overlay: Option<&BigramOverlay>,
    abort_signal: &AtomicBool,
    base_path: &Path,
    arena: crate::simd_path::ArenaPtr,
    overflow_arena: crate::simd_path::ArenaPtr,
) -> GrepResult<'a> {
    let total_files = files.live_count();

    // Extract the grep text and file constraints from the parsed query.
    // For grep, the search pattern is the original query with constraint tokens
    // removed. All non-constraint text tokens are collected and joined with
    // spaces to form the grep pattern:
    //   "name = *.rs someth" -> grep "name = someth" with constraint Extension("rs")
    let constraints_from_query = &query.constraints[..];

    let grep_text = if !matches!(query.fuzzy_query, fff_query_parser::FuzzyQuery::Empty) {
        query.grep_text()
    } else {
        // if constraint-only or empty query we use raw_query for backslash-escape handling
        let t = query.raw_query.trim();
        if t.starts_with('\\') && t.len() > 1 {
            let suffix = &t[1..];
            let parser = QueryParser::new(GrepConfig);
            if !parser.parse(suffix).constraints.is_empty() {
                suffix.to_string()
            } else {
                t.to_string()
            }
        } else {
            t.to_string()
        }
    };

    if grep_text.is_empty() {
        return GrepResult::empty(total_files, total_files);
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
            // Bigram prefilter: pick 5 evenly-spaced probe bigrams, require
            // (5 - max_typos) of them to appear. Widely-spaced probes are
            // far more selective than sliding windows of adjacent bigrams.
            let bigram_candidates = if let Some(idx) = bigram_index
                && idx.is_ready()
            {
                let bq = fuzzy_to_bigram_query(&grep_text, 7);
                if !bq.is_any()
                    && let Some(mut candidates) = bq.evaluate(idx)
                {
                    if let Some(overlay) = bigram_overlay {
                        for (r, t) in candidates.iter_mut().zip(overlay.tombstones().iter()) {
                            *r &= !t;
                        }
                        // Fuzzy: conservatively add all modified files
                        for file_idx in overlay.modified_indices() {
                            let word = file_idx / 64;
                            if word < candidates.len() {
                                candidates[word] |= 1u64 << (file_idx % 64);
                            }
                        }
                    }
                    Some(candidates)
                } else {
                    None
                }
            } else {
                None
            };

            let base_count = match bigram_overlay {
                Some(bigram_overlay) => bigram_overlay.base_file_count(),
                None => files.len(),
            };

            let (mut files_to_search, mut filtered_file_count) = prefilter_files(
                files,
                constraints_from_query,
                bigram_candidates.as_deref(),
                base_count,
                options,
                arena,
                overflow_arena,
            );

            if files_to_search.is_empty()
                && let Some(stripped) =
                    strip_file_path_constraint_if_present(constraints_from_query)
            {
                let (retry_files, retry_count) = prefilter_files(
                    files,
                    &stripped,
                    bigram_candidates.as_deref(),
                    base_count,
                    options,
                    arena,
                    overflow_arena,
                );

                files_to_search = retry_files;
                filtered_file_count = retry_count;
            }

            if files_to_search.is_empty() {
                return GrepResult::empty(total_files, filtered_file_count);
            }

            return super::fuzzy_grep::fuzzy_grep_search(
                &grep_text,
                &files_to_search,
                options,
                total_files,
                filtered_file_count,
                case_insensitive,
                budget,
                abort_signal,
                base_path,
                arena,
                overflow_arena,
            );
        }
        GrepMode::Regex => build_regex(&grep_text, options.smart_case)
            .inspect_err(|err| {
                tracing::warn!("Regex compilation failed for {}. Error {}", grep_text, err);

                regex_fallback_error = Some(err.to_string());
            })
            .ok(),
    };

    let is_multiline = has_unescaped_newline_escape(&grep_text);

    let effective_pattern = if is_multiline {
        replace_unescaped_newline_escapes(&grep_text)
    } else {
        grep_text.to_string()
    };

    let finder_pattern: Vec<u8> = if case_insensitive {
        effective_pattern.as_bytes().to_ascii_lowercase()
    } else {
        effective_pattern.as_bytes().to_vec()
    };
    let finder = memchr::memmem::Finder::new(&finder_pattern);
    let pattern_len = finder_pattern.len() as u32;

    // Bigram prefiltering: query the inverted index + merge overlay.
    // For PlainText mode: extract bigrams directly from the literal pattern.
    // For Regex mode: decompose the regex HIR into an AND/OR bigram query tree
    // and evaluate it against the inverted index (supports alternation, optional
    // groups, character classes, and sparse-1 bigrams across single-byte wildcards).
    let bigram_candidates = if let Some(idx) = bigram_index
        && idx.is_ready()
    {
        let raw_candidates = if regex.is_none() {
            // PlainText or regex-fallback-to-plain: literal bigram query
            idx.query(effective_pattern.as_bytes())
        } else {
            // Regex mode: decompose pattern into bigram query tree
            let bq = regex_to_bigram_query(&effective_pattern);
            if !bq.is_any() { bq.evaluate(idx) } else { None }
        };

        if let Some(mut candidates) = raw_candidates {
            if let Some(overlay) = bigram_overlay {
                // Clear tombstoned (deleted) files from candidates
                for (r, t) in candidates.iter_mut().zip(overlay.tombstones().iter()) {
                    *r &= !t;
                }

                if regex.is_none() {
                    let pattern_bigrams = extract_bigrams(effective_pattern.as_bytes());
                    for file_idx in overlay.query_modified(&pattern_bigrams) {
                        let word = file_idx / 64;
                        if word < candidates.len() {
                            candidates[word] |= 1u64 << (file_idx % 64);
                        }
                    }
                } else {
                    for file_idx in overlay.modified_indices() {
                        let word = file_idx / 64;
                        if word < candidates.len() {
                            candidates[word] |= 1u64 << (file_idx % 64);
                        }
                    }
                }
            }
            Some(candidates)
        } else {
            None
        }
    } else {
        None
    };

    // Bigram bitset only covers `files[..bigram_boundary]`, new files aka overflow
    // (max 1024 always scanned)
    let bigram_boundary = bigram_overlay
        .map(|o| o.base_file_count())
        .unwrap_or(files.len());

    let (mut files_to_search, mut filtered_file_count) = prefilter_files(
        files,
        constraints_from_query,
        bigram_candidates.as_deref(),
        bigram_boundary,
        options,
        arena,
        overflow_arena,
    );

    if files_to_search.is_empty()
        && let Some(stripped) = strip_file_path_constraint_if_present(constraints_from_query)
    {
        let (retry_files, retry_count) = prefilter_files(
            files,
            &stripped,
            bigram_candidates.as_deref(),
            bigram_boundary,
            options,
            arena,
            overflow_arena,
        );
        files_to_search = retry_files;
        filtered_file_count = retry_count;
    }

    if files_to_search.is_empty() {
        return GrepResult::empty(total_files, filtered_file_count);
    }

    // `PlainTextMatcher` is used by the grep-searcher engine for line detection.
    // `PlainTextSink` / `RegexSink` handle highlight extraction independently via ripgrep create
    let plain_matcher = PlainTextMatcher {
        needle: &finder_pattern,
        case_insensitive,
    };

    let searcher = {
        let mut b = SearcherBuilder::new();
        b.line_number(true).multi_line(is_multiline);
        b
    }
    .build();

    let should_prefilter = regex.is_none();
    let mut result = perform_grep(
        &files_to_search,
        options,
        &GrepContext {
            total_files,
            filtered_file_count,
            budget,
            base_path,
            arena,
            overflow_arena,
            prefilter: should_prefilter.then_some(&finder),
            prefilter_case_insensitive: case_insensitive,
            abort_signal,
        },
        |file_bytes: &[u8], max_matches: usize| {
            let state = SinkState {
                file_index: 0,
                matches: Vec::with_capacity(4),
                max_matches,
                before_context: options.before_context,
                after_context: options.after_context,
                classify_definitions: options.classify_definitions,
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
    );
    result.regex_fallback_error = regex_fallback_error;
    result
}

pub fn parse_grep_query(query: &str) -> FFFQuery<'_> {
    let parser = QueryParser::new(GrepConfig);
    parser.parse(query)
}

fn strip_file_path_constraint_if_present<'a>(
    constraints: &[Constraint<'a>],
) -> Option<fff_query_parser::ConstraintVec<'a>> {
    if !constraints
        .iter()
        .any(|c| matches!(c, Constraint::FilePath(_)))
    {
        return None;
    }

    let filtered: fff_query_parser::ConstraintVec<'a> = constraints
        .iter()
        .filter(|c| !matches!(c, Constraint::FilePath(_)))
        .cloned()
        .collect();

    Some(filtered)
}
