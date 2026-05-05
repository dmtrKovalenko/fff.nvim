//! High-performance grep engine for live content search.
//!
//! Searches file contents using the `grep-searcher` crate with mmap-backed
//! file access. Files are searched in frecency order for optimal pagination
//! performance — the most relevant files are searched first, enabling early
//! termination once enough results are collected.

use crate::{
    BigramFilter, BigramOverlay,
    bigram_query::{fuzzy_to_bigram_query, regex_to_bigram_query},
    constraints::apply_constraints,
    extract_bigrams,
    sort_buffer::sort_with_buffer,
    types::{ContentCacheBudget, FileItem},
};
use aho_corasick::AhoCorasick;
pub use fff_grep::{
    Searcher, SearcherBuilder, Sink, SinkMatch,
    lines::{self, LineStep},
    matcher::{Match, Matcher, NoError},
};
use fff_query_parser::{Constraint, FFFQuery, GrepConfig, QueryParser};
use rayon::prelude::*;
use smallvec::SmallVec;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::Level;

/// Detect if a line looks like a code definition (struct, fn, class, etc.).
///
/// Used at match time to tag `GrepMatch::is_definition` so that output
/// formatters can sort/annotate definitions without re-scanning lines.
///
/// Hand-rolled keyword scanner — avoids regex overhead entirely.
/// Strips optional visibility/modifier keywords, then checks if the next
/// token is a definition keyword followed by a word boundary.
pub fn is_definition_line(line: &str) -> bool {
    let s = line.trim_start().as_bytes();
    let s = skip_modifiers(s);
    is_definition_keyword(s)
}

/// Modifier keywords that can precede a definition keyword.
/// Each must be followed by whitespace to be consumed.
const MODIFIERS: &[&[u8]] = &[
    b"pub",
    b"export",
    b"default",
    b"async",
    b"abstract",
    b"unsafe",
    b"static",
    b"protected",
    b"private",
    b"public",
];

/// Definition keywords to detect.
const DEF_KEYWORDS: &[&[u8]] = &[
    b"struct",
    b"fn",
    b"enum",
    b"trait",
    b"impl",
    b"class",
    b"interface",
    b"function",
    b"def",
    b"func",
    b"type",
    b"module",
    b"object",
];

/// Skip zero or more modifier keywords (including `pub(crate)` style visibility).
fn skip_modifiers(mut s: &[u8]) -> &[u8] {
    loop {
        // Handle `pub(...)` — e.g. `pub(crate)`, `pub(super)`
        if s.starts_with(b"pub(")
            && let Some(end) = s.iter().position(|&b| b == b')')
        {
            s = skip_ws(&s[end + 1..]);
            continue;
        }
        let mut matched = false;
        for &kw in MODIFIERS {
            if s.starts_with(kw) {
                let rest = &s[kw.len()..];
                if rest.first().is_some_and(|b| b.is_ascii_whitespace()) {
                    s = skip_ws(rest);
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            return s;
        }
    }
}

/// Check if `s` starts with a definition keyword followed by a word boundary.
fn is_definition_keyword(s: &[u8]) -> bool {
    for &kw in DEF_KEYWORDS {
        if s.starts_with(kw) {
            let after = s.get(kw.len());
            // Word boundary: end of input, or next byte is not alphanumeric/underscore
            if after.is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_') {
                return true;
            }
        }
    }
    false
}

/// Skip ASCII whitespace.
#[inline]
fn skip_ws(s: &[u8]) -> &[u8] {
    let n = s
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(s.len());
    &s[n..]
}

/// Detect import/use lines — lower value than definitions or usages.
///
/// Checks if the line (after leading whitespace) starts with a common
/// import statement prefix. Pure byte-level checks, no regex.
pub fn is_import_line(line: &str) -> bool {
    let s = line.trim_start().as_bytes();
    s.starts_with(b"import ")
        || s.starts_with(b"import\t")
        || (s.starts_with(b"from ") && s.get(5).is_some_and(|&b| b == b'\'' || b == b'"'))
        || s.starts_with(b"use ")
        || s.starts_with(b"use\t")
        || starts_with_require(s)
        || starts_with_include(s)
}

/// Match `require(` or `require (`.
#[inline]
fn starts_with_require(s: &[u8]) -> bool {
    if !s.starts_with(b"require") {
        return false;
    }
    let rest = &s[b"require".len()..];
    rest.first() == Some(&b'(') || (rest.first() == Some(&b' ') && rest.get(1) == Some(&b'('))
}

/// Match `# include ` (with optional spaces after `#`).
#[inline]
fn starts_with_include(s: &[u8]) -> bool {
    if s.first() != Some(&b'#') {
        return false;
    }
    let rest = skip_ws(&s[1..]);
    rest.starts_with(b"include ") || rest.starts_with(b"include\t")
}

/// Determine whether `text` contains any regex metacharacters.
/// Uses `regex::escape` from the regex crate as the source of truth — if the
/// escaped form differs from the original, the text contains characters that
/// would be interpreted as regex syntax. This is deterministic and always in
/// sync with the regex engine (no hand-rolled heuristic to maintain).
///
/// Callers can use this to choose between `GrepMode::Regex` and
/// `GrepMode::PlainText`. When `Regex` mode is chosen and the pattern turns
/// out to be invalid, `grep_search` already falls back to plain-text matching
/// and populates `regex_fallback_error`.
pub fn has_regex_metacharacters(text: &str) -> bool {
    regex::escape(text) != text
}

/// Check if `text` contains `\n` that is NOT preceded by another `\`.
///
/// `\n` → true (user wants multiline search)
/// `\\n` → false (escaped backslash followed by literal `n`, e.g. `\\nvim-data`)
#[inline]
fn has_unescaped_newline_escape(text: &str) -> bool {
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
                // Odd number of backslashes before 'n' → real \n escape
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
/// `\n` → newline character
/// `\\n` → preserved as-is (literal backslash + `n`)
fn replace_unescaped_newline_escapes(text: &str) -> String {
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
            max_file_size: 10 * 1024 * 1024,
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

/// Lightweight wrapper around `regex::bytes::Regex` implementing the
/// `grep_matcher::Matcher` trait required by `grep-searcher`.
///
/// When `is_multiline` is false (the common case), we report `\n` as the
/// line terminator. This enables the **fast** search path in `fff-searcher`:
/// the searcher calls `find()` once on the entire remaining buffer, letting
/// the regex DFA skip non-matching content in a single pass.
///
/// For multiline patterns we must NOT report a line terminator — the regex
/// can match across line boundaries, so the searcher needs the `MultiLine`
/// strategy.
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
            // ASCII case-insensitive: lowercase the haystack slice on the fly.
            // We scan with a rolling window to avoid allocating a full copy.
            ascii_case_insensitive_find(hay, self.needle)
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

/// ASCII case-insensitive substring search.
///
/// Uses a SIMD-accelerated two-byte scan (first + last byte of needle) via
/// `memchr2_iter`, then verifies candidates with a fast byte comparison that
/// leverages the fact that ASCII case differs only in bit 0x20.
#[inline]
fn ascii_case_insensitive_find(haystack: &[u8], needle_lower: &[u8]) -> Option<usize> {
    let nlen = needle_lower.len();
    if nlen == 0 {
        return Some(0);
    }

    if haystack.len() < nlen {
        return None;
    }

    let first_lo = needle_lower[0];
    let first_hi = first_lo.to_ascii_uppercase();

    // Single-byte needle: just find either case variant.
    if nlen == 1 {
        return memchr::memchr2(first_lo, first_hi, haystack);
    }

    let tail = &needle_lower[1..];
    let end = haystack.len() - nlen;

    // Scan for candidates where the first byte matches (either case).
    for pos in memchr::memchr2_iter(first_lo, first_hi, &haystack[..=end]) {
        // Verify the remaining bytes with bitwise ASCII case-insensitive compare.
        // For ASCII letters, (a ^ b) & ~0x20 == 0 when they match ignoring case.
        // For non-letters, exact equality is required; OR-ing with 0x20 maps both
        // cases to lowercase and is correct for non-alpha bytes that are already equal.
        let candidate = unsafe { haystack.get_unchecked(pos + 1..pos + nlen) };
        if ascii_case_eq(candidate, tail) {
            return Some(pos);
        }
    }
    None
}

/// Fast ASCII case-insensitive byte slice comparison.
///
/// Returns true if `a` and `b` are equal when compared case-insensitively
/// for ASCII bytes. Both slices must have the same length.
#[inline]
fn ascii_case_eq(a: &[u8], b: &[u8]) -> bool {
    debug_assert_eq!(a.len(), b.len());
    // Process 8 bytes at a time using u64 bitwise operations.
    // For each byte: (x | 0x20) maps uppercase ASCII to lowercase.
    // This is correct for letters. For non-letter bytes where the original
    // values are equal, OR-ing with 0x20 preserves equality. For non-letter
    // bytes where values differ, this can produce false positives only when
    // they differ exactly by 0x20 — we do a fast exact-match check first
    // to catch those rare cases.
    let len = a.len();
    let mut i = 0;

    // Fast path: compare 8 bytes at a time
    while i + 8 <= len {
        let va = u64::from_ne_bytes(unsafe { *(a.as_ptr().add(i) as *const [u8; 8]) });
        let vb = u64::from_ne_bytes(unsafe { *(b.as_ptr().add(i) as *const [u8; 8]) });

        // Quick exact-match shortcut (common for non-alpha content)
        if va != vb {
            // Case-insensitive: OR each byte with 0x20 to fold case
            const MASK: u64 = 0x2020_2020_2020_2020;
            if (va | MASK) != (vb | MASK) {
                return false;
            }
        }
        i += 8;
    }

    // Handle remaining bytes
    while i < len {
        let ha = unsafe { *a.get_unchecked(i) };
        let hb = unsafe { *b.get_unchecked(i) };
        if ha != hb && (ha | 0x20) != (hb | 0x20) {
            return false;
        }
        i += 1;
    }

    true
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
        let trimmed_len = {
            let mut len = line_bytes.len();
            while len > 0 && matches!(line_bytes[len - 1], b'\n' | b'\r') {
                len -= 1;
            }
            len
        };
        let trimmed_bytes = &line_bytes[..trimmed_len];

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
        let is_definition = self.classify_definitions && is_definition_line(&line_content);
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
fn truncate_display_bytes(bytes: &[u8]) -> &[u8] {
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
    let total_files = files.len();

    if patterns.is_empty() || patterns.iter().all(|p| p.is_empty()) {
        return GrepResult {
            total_files,
            filtered_file_count: total_files,
            ..Default::default()
        };
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

    let (mut files_to_search, mut filtered_file_count) =
        prepare_files_to_search(files, constraints, options, arena);

    // If constraints yielded 0 files and we had FilePath constraints,
    // retry without them (the path token was likely part of the search text).
    if files_to_search.is_empty()
        && let Some(stripped) = strip_file_path_constraints(constraints)
    {
        let (retry_files, retry_count) = prepare_files_to_search(files, &stripped, options, arena);
        files_to_search = retry_files;
        filtered_file_count = retry_count;
    }

    // Apply bigram prefilter to the file list. Bigram columns only cover
    // the indexable region — `bigram_overlay.base_file_count()` is the
    // authoritative boundary. Files past it (unindexable base, overflow)
    // are always retained and searched directly.
    if let Some(ref candidates) = bigram_candidates {
        let base_ptr = files.as_ptr();
        let bigram_boundary = bigram_overlay
            .map(|o| o.base_file_count())
            .unwrap_or(files.len());

        files_to_search.retain(|f| {
            if f.is_overflow() {
                return true;
            }
            let file_idx = unsafe { (*f as *const FileItem).offset_from(base_ptr) as usize };
            if file_idx >= bigram_boundary {
                return true;
            }
            BigramFilter::is_candidate(candidates, file_idx)
        });
    }

    if files_to_search.is_empty() {
        return GrepResult {
            total_files,
            filtered_file_count,
            ..Default::default()
        };
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

use crate::case_insensitive_memmem;

/// Minimum chunk size for paginated search. Must be large enough for good
/// thread utilization across rayon's pool (~28 threads on modern hardware)
/// but small enough to allow early termination after few chunks.
const PAGINATED_CHUNK_SIZE: usize = 512;

#[tracing::instrument(skip_all, level = Level::DEBUG, fields(prefiltered_count = files_to_search.len()))]
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

    // For paginated searches, process files in chunks to enable early
    // termination. Each chunk is searched in parallel with rayon; between
    // chunks we check whether enough matches have been collected.
    //
    // For full searches (page_limit = MAX), one chunk = all files — same
    // throughput as before, no overhead from the chunking loop.
    //
    // For common queries ("x", "if") with ~99% hit rate: the first 512-file
    // chunk yields ~500 matches, far exceeding page_limit=50. We stop after
    // one chunk (~1ms) instead of searching all 93K files (~175ms).
    let chunk_size = if page_limit < usize::MAX {
        PAGINATED_CHUNK_SIZE
    } else {
        files_to_search.len().max(1)
    };

    let mut result_files: Vec<&'a FileItem> = Vec::new();
    let mut all_matches: Vec<GrepMatch> = Vec::new();
    let mut files_consumed: usize = 0;
    let mut page_filled = false;

    for chunk in files_to_search.chunks(chunk_size) {
        let chunk_offset = files_consumed;

        // Parallel phase: search all files in this chunk concurrently.
        // Within a chunk every file is visited (no gaps), so pagination
        // offsets remain correct across chunk boundaries.
        let chunk_results: Vec<(usize, &'a FileItem, Vec<GrepMatch>)> = chunk
            .par_iter()
            .enumerate()
            .map_init(
                // allocatge a single reusable buffer per thread
                || Vec::with_capacity(64 * 1024),
                |buf, (local_idx, file)| {
                    if ctx.abort_signal.load(Ordering::Relaxed) {
                        budget_exceeded.store(true, Ordering::Relaxed);
                        return None;
                    }

                    if let Some(budget) = time_budget
                        && all_matches.len() > 1
                        && search_start.elapsed() > budget
                    {
                        budget_exceeded.store(true, Ordering::Relaxed);
                        return None;
                    }

                    let content = file.get_content_for_search(
                        buf,
                        ctx.arena_for_file(file),
                        ctx.base_path,
                        ctx.budget,
                    )?;

                    // Fast whole-file memmem check before entering the
                    // grep-searcher machinery. Skips Vec alloc, Searcher
                    // setup, and line-splitting for files that can't match.
                    if let Some(pf) = ctx.prefilter {
                        let found = if ctx.prefilter_case_insensitive {
                            case_insensitive_memmem::search_packed_pair(content, pf.needle())
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

/// Flatten per-file results into the final `GrepResult`.
///
/// Shared post-processing for both `run_file_search` (simple closure) and
/// `fuzzy_grep_search` (which uses `map_init` for per-thread matcher reuse).
fn collect_grep_results<'a>(
    per_file_results: Vec<(usize, &'a FileItem, Vec<GrepMatch>)>,
    files_to_search_len: usize,
    options: &GrepSearchOptions,
    total_files: usize,
    filtered_file_count: usize,
    budget_exceeded: bool,
) -> GrepResult<'a> {
    let page_limit = options.page_limit;

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
            if options.trim_whitespace {
                m.trim_leading_whitespace();
            }
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
        files_consumed = files_to_search_len;
    }

    let has_more = budget_exceeded
        || (all_matches.len() >= page_limit && files_consumed < files_to_search_len);

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
        total_files,
        filtered_file_count,
        next_file_offset,
        regex_fallback_error: None,
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
    arena: crate::simd_path::ArenaPtr,
) -> (Vec<&'a FileItem>, usize) {
    let prefiltered: Vec<&FileItem> = if constraints.is_empty() {
        files
            .iter()
            .filter(|f| {
                !f.is_deleted() && !f.is_binary() && f.size > 0 && f.size <= options.max_file_size
            })
            .collect()
    } else {
        match apply_constraints(files, constraints, arena) {
            Some(constrained) => constrained
                .into_iter()
                .filter(|f| {
                    !f.is_deleted()
                        && !f.is_binary()
                        && f.size > 0
                        && f.size <= options.max_file_size
                })
                .collect(),
            None => files
                .iter()
                .filter(|f| {
                    !f.is_deleted()
                        && !f.is_binary()
                        && f.size > 0
                        && f.size <= options.max_file_size
                })
                .collect(),
        }
    };

    let total_count = prefiltered.len();
    let mut sorted_files = prefiltered;

    // Only sort when there is meaningful frecency or modification data to rank by.
    // On large repos (500k+ files) with no frecency data (fresh session, benchmark),
    // skipping the O(n log n) sort saves ~200ms per query.
    let needs_sort = sorted_files
        .iter()
        .any(|f| f.total_frecency_score() != 0 || f.modified != 0);

    if needs_sort {
        sort_with_buffer(&mut sorted_files, |a, b| {
            b.total_frecency_score()
                .cmp(&a.total_frecency_score())
                .then(b.modified.cmp(&a.modified))
        });
    }

    if options.file_offset > 0 && options.file_offset < total_count {
        let paginated = sorted_files.split_off(options.file_offset);
        (paginated, total_count)
    } else if options.file_offset >= total_count {
        (Vec::new(), total_count)
    } else {
        // offset == 0: no split needed, return as-is
        (sorted_files, total_count)
    }
}

/// Fuzzy grep search using SIMD-accelerated `neo_frizbee::match_list`.
///
/// Why this doesn't use `grep-searcher` / `GrepSink`
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
#[allow(clippy::too_many_arguments)]
fn fuzzy_grep_search<'a>(
    grep_text: &str,
    files_to_search: &[&'a FileItem],
    options: &GrepSearchOptions,
    total_files: usize,
    filtered_file_count: usize,
    case_insensitive: bool,
    budget: &ContentCacheBudget,
    abort_signal: &AtomicBool,
    base_path: &Path,
    arena: crate::simd_path::ArenaPtr,
    _overflow_arena: crate::simd_path::ArenaPtr,
) -> GrepResult<'a> {
    // max_typos controls how many *needle* characters can be unmatched.
    // A transposition (e.g. "shcema" → "schema") costs ~1 typo with
    // default gap penalties. We scale max_typos by needle length:
    //   1-2 chars → 0 typos (exact subsequence only)
    //   3-5 chars → 1 typo
    //   6+  chars → 2 typos
    // Cap at 2: higher values (3+) let the SIMD prefilter pass lines
    // missing key characters entirely (e.g. query "flvencodeX" matching
    // lines without 'l' or 'v'). Quality comes from the post-match filters.
    let max_typos = (grep_text.len() / 3).min(2);
    let scoring = neo_frizbee::Scoring {
        // Use default gap penalties. Higher values (e.g. 20) cause
        // smith-waterman to prefer *dropping needle chars* over paying
        // gap costs, which inflates the typo count and breaks
        // transposition matching ("shcema" → "schema" becomes 3 typos instead of 1)
        exact_match_bonus: 100,
        // gap_open_penalty: 4,
        // gap_extend_penalty: 2,
        prefix_bonus: 0,
        capitalization_bonus: if case_insensitive { 0 } else { 4 },
        ..neo_frizbee::Scoring::default()
    };

    let matcher = neo_frizbee::Matcher::new(
        grep_text,
        &neo_frizbee::Config {
            // Use the real max_typos so frizbee's SIMD prefilter actually rejects non-matching lines (~2 SIMD instructions per line vs full SW scoring).
            max_typos: Some(max_typos as u16),
            sort: false,
            scoring,
        },
    );

    // Minimum score threshold: 50% of a perfect contiguous match.
    // With default scoring (match_score=12, matching_case_bonus=4 = 16/char),
    // a transposition costs ~5 from a gap, keeping the score well above 50%.
    let perfect_score = (grep_text.len() as u16) * 16;
    let min_score = (perfect_score * 50) / 100;

    // Target identifiers are often longer than the query due to delimiters
    // (e.g. query "flvencodepicture" → "ff_flv_encode_picture_header").
    // Allow 3x needle length to accommodate underscore/dot-separated names.
    let max_match_span = grep_text.len() * 3;
    let needle_len = grep_text.len();

    // Each delimiter (_, .) in the target creates a gap. A typical C/Rust
    // identifier like "ff_flv_encode_picture_header" has 4-5 underscores.
    // Scale generously so delimiter gaps don't reject valid matches.
    let max_gaps = (needle_len / 3).max(2);

    // File-level prefilter: collect unique needle chars (both cases) for
    // a fast memchr scan.  If a file doesn't contain enough distinct
    // needle characters, skip it entirely — no line splitting needed.
    let needle_bytes = grep_text.as_bytes();
    let mut unique_needle_chars: Vec<u8> = Vec::new();
    for &b in needle_bytes {
        let lo = b.to_ascii_lowercase();
        let hi = b.to_ascii_uppercase();
        if !unique_needle_chars.contains(&lo) {
            unique_needle_chars.push(lo);
        }
        if lo != hi && !unique_needle_chars.contains(&hi) {
            unique_needle_chars.push(hi);
        }
    }

    // How many distinct needle chars must appear in the file.
    // With max_typos allowed, we need at least (unique_count - max_typos).
    let unique_count = {
        let mut seen = [false; 256];
        for &b in needle_bytes {
            seen[b.to_ascii_lowercase() as usize] = true;
        }
        seen.iter().filter(|&&v| v).count()
    };
    let min_chars_required = unique_count.saturating_sub(max_typos);

    let time_budget = if options.time_budget_ms > 0 {
        Some(std::time::Duration::from_millis(options.time_budget_ms))
    } else {
        None
    };
    let search_start = std::time::Instant::now();
    let budget_exceeded = AtomicBool::new(false);
    let max_matches_per_file = options.max_matches_per_file;
    // Parallel phase with `map_init`: each rayon worker thread clones the
    // matcher once and gets a reusable read buffer. The buffer avoids
    // mmap/munmap syscalls for non-cached files.
    let per_file_results: Vec<(usize, &'a FileItem, Vec<GrepMatch>)> = files_to_search
        .par_iter()
        .enumerate()
        .map_init(
            || (matcher.clone(), Vec::with_capacity(64 * 1024)),
            |(matcher, buf), (idx, file)| {
                if abort_signal.load(Ordering::Relaxed) {
                    budget_exceeded.store(true, Ordering::Relaxed);
                    return None;
                }

                if let Some(budget) = time_budget
                    && search_start.elapsed() > budget
                {
                    budget_exceeded.store(true, Ordering::Relaxed);
                    return None;
                }

                let file_bytes = file.get_content_for_search(buf, arena, base_path, budget)?;

                // File-level prefilter: check if enough distinct needle chars
                // exist anywhere in the file bytes.  Uses memchr for speed.
                if min_chars_required > 0 {
                    let mut chars_found = 0usize;
                    for &ch in &unique_needle_chars {
                        if memchr::memchr(ch, file_bytes).is_some() {
                            chars_found += 1;
                            if chars_found >= min_chars_required {
                                break;
                            }
                        }
                    }
                    if chars_found < min_chars_required {
                        return None;
                    }
                }

                // Validate the whole file as UTF-8 once upfront. Source code
                // files are virtually always valid UTF-8; this single check
                // replaces per-line from_utf8 calls (~8% of fuzzy grep time).
                let file_is_utf8 = std::str::from_utf8(file_bytes).is_ok();

                // Reuse grep-searcher's LineStep for SIMD-accelerated line iteration.
                let mut stepper = LineStep::new(b'\n', 0, file_bytes.len());
                let estimated_lines = (file_bytes.len() / 40).max(64);
                let mut file_lines: Vec<&str> = Vec::with_capacity(estimated_lines);
                let mut line_meta: Vec<(u64, u64)> = Vec::with_capacity(estimated_lines);
                let line_term_lf = fff_grep::LineTerminator::byte(b'\n');
                let line_term_cr = fff_grep::LineTerminator::byte(b'\r');

                let mut line_number: u64 = 1;
                while let Some(line_match) = stepper.next_match(file_bytes) {
                    let byte_offset = line_match.start() as u64;

                    // Strip line terminators (\n, \r).
                    let trimmed = lines::without_terminator(
                        lines::without_terminator(&file_bytes[line_match], line_term_lf),
                        line_term_cr,
                    );

                    if !trimmed.is_empty() {
                        // SAFETY: when the whole file is valid UTF-8, every
                        // sub-slice split on ASCII byte boundaries (\n, \r)
                        // is also valid UTF-8.
                        let line_str = if file_is_utf8 {
                            unsafe { std::str::from_utf8_unchecked(trimmed) }
                        } else if let Ok(s) = std::str::from_utf8(trimmed) {
                            s
                        } else {
                            line_number += 1;
                            continue;
                        };
                        file_lines.push(line_str);
                        line_meta.push((line_number, byte_offset));
                    }

                    line_number += 1;
                }

                if file_lines.is_empty() {
                    return None;
                }

                // Single-pass: score + indices in one Smith-Waterman run per line.
                let matches_with_indices = matcher.match_list_indices(&file_lines);
                let mut file_matches: Vec<GrepMatch> = Vec::new();

                for mut match_indices in matches_with_indices {
                    if match_indices.score < min_score {
                        continue;
                    }

                    let idx = match_indices.index as usize;
                    let raw_line = file_lines[idx];

                    let truncated = truncate_display_bytes(raw_line.as_bytes());
                    let display_line = if truncated.len() < raw_line.len() {
                        // SAFETY: truncate_display_bytes preserves UTF-8 char boundaries
                        &raw_line[..truncated.len()]
                    } else {
                        raw_line
                    };

                    // If the line was truncated, re-compute indices on the shorter string.
                    if display_line.len() < raw_line.len() {
                        let Some(re_indices) = matcher
                            .match_list_indices(&[display_line])
                            .into_iter()
                            .next()
                        else {
                            continue;
                        };
                        match_indices = re_indices;
                    }

                    // upstream returns indices in reverse order, sort ascending
                    match_indices.indices.sort_unstable();

                    // Minimum matched chars: at least (needle_len - max_typos)
                    // characters must appear. This is consistent with the typo
                    // budget: each typo can drop one needle char from the alignment.
                    let min_matched = needle_len.saturating_sub(max_typos).max(1);
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
                        // present), slightly relaxed for typo matches to handle
                        // delimiter-heavy targets (e.g. "ff_flv_encode_picture_header"
                        // has span inflated by underscores → density ~68%).
                        let density = (indices.len() * 100) / span;
                        let min_density = if indices.len() >= needle_len {
                            45 // Perfect subsequence — relaxed (delimiters inflate span)
                        } else {
                            65 // Has typos — moderately strict
                        };
                        if density < min_density {
                            continue;
                        }

                        // Gap count check: count discontinuities in the indices.
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
                        file_index: 0,
                        line_number: ln,
                        col,
                        byte_offset: bo,
                        is_definition: options.classify_definitions
                            && is_definition_line(display_line),
                        line_content: display_line.to_string(),
                        match_byte_offsets,
                        fuzzy_score: Some(match_indices.score),
                        context_before: Vec::new(),
                        context_after: Vec::new(),
                    });

                    if max_matches_per_file != 0 && file_matches.len() >= max_matches_per_file {
                        break;
                    }
                }

                if file_matches.is_empty() {
                    return None;
                }

                Some((idx, *file, file_matches))
            },
        )
        .flatten()
        .collect();

    collect_grep_results(
        per_file_results,
        files_to_search.len(),
        options,
        total_files,
        filtered_file_count,
        budget_exceeded.load(Ordering::Relaxed),
    )
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
    let total_files = files.len();

    // Extract the grep text and file constraints from the parsed query.
    // For grep, the search pattern is the original query with constraint tokens
    // removed. All non-constraint text tokens are collected and joined with
    // spaces to form the grep pattern:
    //   "name = *.rs someth" -> grep "name = someth" with constraint Extension("rs")
    let constraints_from_query = &query.constraints[..];

    let grep_text = if !matches!(query.fuzzy_query, fff_query_parser::FuzzyQuery::Empty) {
        query.grep_text()
    } else {
        // Constraint-only or empty query — use raw_query for backslash-escape handling.
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
        return GrepResult {
            total_files,
            filtered_file_count: total_files,
            next_file_offset: 0,
            matches: Vec::with_capacity(4),
            files: Vec::new(),
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
            let (mut files_to_search, mut filtered_file_count) =
                prepare_files_to_search(files, constraints_from_query, options, arena);

            if files_to_search.is_empty()
                && let Some(stripped) = strip_file_path_constraints(constraints_from_query)
            {
                let (retry_files, retry_count) =
                    prepare_files_to_search(files, &stripped, options, arena);
                files_to_search = retry_files;
                filtered_file_count = retry_count;
            }

            if files_to_search.is_empty() {
                return GrepResult {
                    total_files,
                    filtered_file_count,
                    next_file_offset: 0,
                    ..Default::default()
                };
            }

            // Bigram prefilter: pick 5 evenly-spaced probe bigrams, require
            // (5 - max_typos) of them to appear. Widely-spaced probes are
            // far more selective than sliding windows of adjacent bigrams.
            if let Some(idx) = bigram_index
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

                    // Bigram columns only cover the indexable region at
                    // `files[..overlay.base_file_count()]` — which equals
                    // `indexable_count` today. Files past that boundary
                    // (unindexable base files, overflow) are not tracked by
                    // the bigram filter, so we always retain them and let
                    // the full text search decide.
                    let base_ptr = files.as_ptr();
                    let bigram_boundary = bigram_overlay
                        .map(|o| o.base_file_count())
                        .unwrap_or(files.len());

                    files_to_search.retain(|f| {
                        if f.is_overflow() {
                            return true;
                        }

                        let file_idx =
                            unsafe { (*f as *const FileItem).offset_from(base_ptr) as usize };

                        if file_idx >= bigram_boundary {
                            return true;
                        }

                        BigramFilter::is_candidate(&candidates, file_idx)
                    });
                }
            }

            return fuzzy_grep_search(
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

    // Build the finder pattern once — used by PlainTextSink (and as a
    // literal-needle fallback anchor when regex compilation fell back to plain).
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

    // Overflow files (added after the bigram index was built) are not in
    // the candidate bitset. They're few by definition, so just search all
    // of them directly via memchr — no bigram tracking needed.
    let overflow_start = bigram_overlay
        .map(|o| o.base_file_count())
        .unwrap_or(files.len());

    // it is important that this step is coming as early as possible
    let (files_to_search, filtered_file_count) = match bigram_candidates {
        Some(ref candidates) if constraints_from_query.is_empty() => {
            // this call is essentially free and much more efficient than allowing a recollection
            let overflow_count = files.len().saturating_sub(overflow_start);
            let cap = BigramFilter::count_candidates(candidates) + overflow_count;
            let mut result: Vec<&FileItem> = Vec::with_capacity(cap);

            for (word_idx, &word) in candidates.iter().enumerate() {
                if word == 0 {
                    continue;
                }
                let base = word_idx * 64;
                let mut bits = word;
                while bits != 0 {
                    let bit = bits.trailing_zeros() as usize;
                    let file_idx = base + bit;
                    // Stop at the overflow boundary: the loop below walks
                    // every overflow file, so counting them here too would duplicate.
                    if file_idx < overflow_start {
                        let f = unsafe { files.get_unchecked(file_idx) };
                        if !f.is_binary() && f.size <= options.max_file_size {
                            result.push(f);
                        }
                    }
                    bits &= bits - 1;
                }
            }

            // Append all overflow files — they're not in the bigram index
            // so we search them unconditionally (typically few files).
            for f in &files[overflow_start..] {
                if !f.is_binary() && !f.is_deleted() && f.size <= options.max_file_size {
                    result.push(f);
                }
            }

            let total_searchable = files.len();
            let needs_sort = result
                .iter()
                .any(|f| f.total_frecency_score() != 0 || f.modified != 0);

            if needs_sort {
                sort_with_buffer(&mut result, |a, b| {
                    b.total_frecency_score()
                        .cmp(&a.total_frecency_score())
                        .then(b.modified.cmp(&a.modified))
                });
            }

            if options.file_offset > 0 && options.file_offset < result.len() {
                let paginated = result.split_off(options.file_offset);
                (paginated, total_searchable)
            } else if options.file_offset >= result.len() {
                (Vec::new(), total_searchable)
            } else {
                (result, total_searchable)
            }
        }
        _ => {
            let (mut fts, mut fc) =
                prepare_files_to_search(files, constraints_from_query, options, arena);

            if fts.is_empty()
                && let Some(stripped) = strip_file_path_constraints(constraints_from_query)
            {
                let (retry_files, retry_count) =
                    prepare_files_to_search(files, &stripped, options, arena);
                fts = retry_files;
                fc = retry_count;
            }

            if let Some(ref candidates) = bigram_candidates {
                let base_ptr = files.as_ptr();
                fts.retain(|f| {
                    if f.is_overflow() {
                        return true;
                    }

                    // we use ptr offsets to avoid additional allocations and keep the index
                    let file_idx =
                        unsafe { (*f as *const FileItem).offset_from(base_ptr) as usize };
                    BigramFilter::is_candidate(candidates, file_idx)
                });
            }

            (fts, fc)
        }
    };

    if files_to_search.is_empty() {
        return GrepResult {
            total_files,
            filtered_file_count,
            next_file_offset: 0,
            ..Default::default()
        };
    }

    // `PlainTextMatcher` is used by the grep-searcher engine for line detection.
    // `PlainTextSink` / `RegexSink` handle highlight extraction independently.
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

fn strip_file_path_constraints<'a>(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unescaped_newline_detection() {
        // Single \n → multiline
        assert!(has_unescaped_newline_escape("foo\\nbar"));
        // \\n → escaped backslash + literal n, NOT multiline
        // (this is what the user types when grepping Rust source with `\\nvim`)
        assert!(!has_unescaped_newline_escape("foo\\\\nvim-data"));
        // Real-world: source file has literal \\AppData\\Local\\nvim-data
        // (double backslash in the file, so user types double backslash)
        assert!(!has_unescaped_newline_escape(
            r#"format!("{}\\AppData\\Local\\nvim-data","#
        ));
        // No \n at all
        assert!(!has_unescaped_newline_escape("hello world"));
        // \\\\n → even number of backslashes before n → NOT multiline
        assert!(!has_unescaped_newline_escape("foo\\\\\\\\nbar"));
        // \\\n → 3 backslashes: first two pair up, third + n = \n → multiline
        assert!(has_unescaped_newline_escape("foo\\\\\\nbar"));
    }

    #[test]
    fn test_replace_unescaped_newline() {
        // \n → real newline
        assert_eq!(replace_unescaped_newline_escapes("foo\\nbar"), "foo\nbar");
        // \\n → preserved as-is
        assert_eq!(
            replace_unescaped_newline_escapes("foo\\\\nvim"),
            "foo\\\\nvim"
        );
    }

    #[test]
    fn test_fuzzy_typo_scoring() {
        // Mirror the config from fuzzy_grep_search
        let needle = "schema";
        let max_typos = (needle.len() / 3).min(2); // 2
        let config = neo_frizbee::Config {
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
            let Some(mut mi) = neo_frizbee::match_list_indices(n, &[h], &config)
                .into_iter()
                .next()
            else {
                return false;
            };
            // upstream returns indices in reverse order, sort ascending
            mi.indices.sort_unstable();
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

    #[test]
    fn test_multi_grep_search() {
        use crate::file_picker::{FilePicker, FilePickerOptions};
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();

        // File 1: has "GrepMode" and "GrepMatch"
        {
            let mut f = std::fs::File::create(dir.path().join("grep.rs")).unwrap();
            writeln!(f, "pub enum GrepMode {{").unwrap();
            writeln!(f, "    PlainText,").unwrap();
            writeln!(f, "    Regex,").unwrap();
            writeln!(f, "}}").unwrap();
            writeln!(f, "pub struct GrepMatch {{").unwrap();
            writeln!(f, "    pub line_number: u64,").unwrap();
            writeln!(f, "}}").unwrap();
        }

        // File 2: has "PlainTextMatcher" only
        {
            let mut f = std::fs::File::create(dir.path().join("matcher.rs")).unwrap();
            writeln!(f, "struct PlainTextMatcher {{").unwrap();
            writeln!(f, "    needle: Vec<u8>,").unwrap();
            writeln!(f, "}}").unwrap();
        }

        // File 3: no matches
        {
            let mut f = std::fs::File::create(dir.path().join("other.rs")).unwrap();
            writeln!(f, "fn main() {{").unwrap();
            writeln!(f, "    println!(\"hello\");").unwrap();
            writeln!(f, "}}").unwrap();
        }

        let mut picker = FilePicker::new(FilePickerOptions {
            base_path: dir.path().to_str().unwrap().into(),
            watch: false,
            ..Default::default()
        })
        .unwrap();
        picker.collect_files().unwrap();

        let files = picker.get_files();
        let arena = picker.arena_base_ptr();

        let options = super::GrepSearchOptions {
            max_file_size: 10 * 1024 * 1024,
            max_matches_per_file: 0,
            smart_case: true,
            file_offset: 0,
            page_limit: 100,
            mode: super::GrepMode::PlainText,
            time_budget_ms: 0,
            before_context: 0,
            after_context: 0,
            classify_definitions: false,
            trim_whitespace: false,
            abort_signal: None,
        };
        let no_cancel = AtomicBool::new(false);

        // Test with 3 patterns
        let result = super::multi_grep_search(
            files,
            &["GrepMode", "GrepMatch", "PlainTextMatcher"],
            &[],
            &options,
            picker.cache_budget(),
            None,
            None,
            &no_cancel,
            dir.path(),
            arena,
            arena,
        );

        assert!(
            result.matches.len() >= 3,
            "Expected at least 3 matches, got {}",
            result.matches.len()
        );

        let has_grep_mode = result
            .matches
            .iter()
            .any(|m| m.line_content.contains("GrepMode"));
        let has_grep_match = result
            .matches
            .iter()
            .any(|m| m.line_content.contains("GrepMatch"));
        let has_plain_text_matcher = result
            .matches
            .iter()
            .any(|m| m.line_content.contains("PlainTextMatcher"));

        assert!(has_grep_mode, "Should find GrepMode");
        assert!(has_grep_match, "Should find GrepMatch");
        assert!(has_plain_text_matcher, "Should find PlainTextMatcher");

        assert_eq!(result.files.len(), 2, "Should match exactly 2 files");

        // Test with single pattern
        let result2 = super::multi_grep_search(
            files,
            &["PlainTextMatcher"],
            &[],
            &options,
            picker.cache_budget(),
            None,
            None,
            &no_cancel,
            dir.path(),
            arena,
            arena,
        );
        assert_eq!(
            result2.matches.len(),
            1,
            "Single pattern should find 1 match"
        );

        // Test with empty patterns
        let result3 = super::multi_grep_search(
            files,
            &[],
            &[],
            &options,
            picker.cache_budget(),
            None,
            None,
            &no_cancel,
            dir.path(),
            arena,
            arena,
        );
        assert_eq!(
            result3.matches.len(),
            0,
            "Empty patterns should find nothing"
        );
    }

    /// Regression test for issue #407: Live grep returns duplicate results
    /// when the bigram candidate bitset has trailing bits set beyond
    /// `base_file_count`. The bitset is rounded up to a multiple of 64 bits
    /// so any trailing bit that happens to be set (e.g. from overlay data)
    /// would previously map to an overflow file index, which was then also
    /// unconditionally appended by the overflow loop, producing duplicates.
    #[test]
    fn test_grep_no_duplicates_with_overflow_trailing_bits() {
        use crate::bigram_filter::{BigramIndexBuilder, BigramOverlay};
        use crate::file_picker::{FilePicker, FilePickerOptions};
        use std::io::Write;
        use std::sync::atomic::AtomicBool;

        let dir = tempfile::tempdir().unwrap();
        // Match the picker's internal dunce-canonicalize so paths passed to
        // on_create_or_modify resolve back to the same base_path on Windows.
        let base = crate::path_utils::canonicalize(dir.path()).unwrap();

        // Five base files: only three contain the pattern "unicorn".
        // We need some files WITHOUT the pattern so the bigrams for
        // "unicorn" aren't treated as ubiquitous (≥90% of files) and
        // dropped from the index during compress().
        let base_contents: &[(&str, &str)] = &[
            ("a.txt", "hello unicorn world"),
            ("b.txt", "another unicorn line"),
            ("c.txt", "one more unicorn here"),
            ("d.txt", "nothing special in here"),
            ("e.txt", "just some random content"),
        ];
        for (name, content) in base_contents {
            let mut f = std::fs::File::create(base.join(name)).unwrap();
            writeln!(f, "{}", content).unwrap();
        }

        let mut picker = FilePicker::new(FilePickerOptions {
            base_path: base.to_str().unwrap().into(),
            watch: false,
            ..Default::default()
        })
        .unwrap();
        picker.collect_files().unwrap();
        assert_eq!(picker.get_files().len(), 5);

        // Manually build a bigram index over the 5 base files.
        let base_count = 5usize;
        let consec_builder = BigramIndexBuilder::new(base_count);
        let skip_builder = BigramIndexBuilder::new(base_count);
        for (i, (_, content)) in base_contents.iter().enumerate() {
            consec_builder.add_file_content(&skip_builder, i, content.as_bytes());
        }
        let mut index = consec_builder.compress(Some(0));
        index.set_skip_index(skip_builder.compress(Some(0)));
        picker.set_bigram_index(index, BigramOverlay::new(base_count));

        // Add three overflow files (new after the bigram index was built),
        // all containing "unicorn".
        for name in ["f.txt", "g.txt", "h.txt"] {
            let path = base.join(name);
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(f, "overflow unicorn entry").unwrap();
            drop(f);
            picker.on_create_or_modify(&path);
        }
        assert_eq!(picker.get_files().len(), 8);

        // Inject a trailing bit into the overlay at a file index that
        // corresponds to an overflow file (i.e. >= base_file_count=5 but
        // < bitset_word_size=64). Without the fix, the bigram-candidate
        // merge would set this bit in the bitset, and the bitset loop would
        // push files[6] while the overflow loop also appends files[5..]
        // which includes files[6], producing a duplicate.
        let overflow_rel = "g.txt"; // middle overflow file
        let overflow_abs = picker
            .get_files()
            .iter()
            .position(|f| f.relative_path(&picker) == overflow_rel)
            .expect("overflow file should be present");
        assert!(overflow_abs >= base_count);
        assert!(
            overflow_abs < 64,
            "index must fit in the single bitset word"
        );

        if let Some(overlay) = picker.bigram_overlay() {
            overlay
                .write()
                .modify_file(overflow_abs, b"overflow unicorn entry");
        }

        // Run a grep for "unicorn": six files match
        // (a, b, c in base + f, g, h in overflow).
        let query = super::parse_grep_query("unicorn");
        let options = super::GrepSearchOptions {
            max_file_size: 10 * 1024 * 1024,
            max_matches_per_file: 0,
            smart_case: true,
            file_offset: 0,
            page_limit: 100,
            mode: super::GrepMode::PlainText,
            time_budget_ms: 0,
            before_context: 0,
            after_context: 0,
            classify_definitions: false,
            trim_whitespace: false,
            abort_signal: Some(std::sync::Arc::new(AtomicBool::new(false))),
        };
        let result = picker.grep(&query, &options);

        // Collect the matched relative paths via the returned files list.
        let mut paths: Vec<String> = result
            .files
            .iter()
            .map(|f| f.relative_path(&picker))
            .collect();
        paths.sort();

        // Every file (base + overflow) should match exactly once.
        let mut dedup = paths.clone();
        dedup.dedup();
        assert_eq!(
            dedup, paths,
            "grep must not return duplicate results (issue #407): {:?}",
            paths
        );
        assert_eq!(
            paths,
            vec!["a.txt", "b.txt", "c.txt", "f.txt", "g.txt", "h.txt"],
        );

        // And the match count must equal the number of files (one line per
        // file). A duplicate entry in files_to_search would double-count
        // matches for the duplicated file.
        assert_eq!(
            result.matches.len(),
            6,
            "expected exactly one match per file, got {}",
            result.matches.len()
        );
    }
}
