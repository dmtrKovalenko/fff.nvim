//! Stable accessor functions for `fff-c` FFI struct fields.
//!
//! # Why this exists
//!
//! `fff-c` exposes its result types as plain `#[repr(C)]` structs. External
//! consumers (Emacs Lisp via `emacs-ffi`, Python `ctypes`, etc.) that access
//! fields by hardcoding byte offsets break silently whenever the struct layout
//! changes — a new field shifts every subsequent offset with no compile-time
//! warning.
//!
//! These functions turn field access into a **stable named API**: callers bind
//! to a symbol name once and are fully insulated from layout changes.
//!
//! # Usage from Emacs Lisp (example)
//!
//! ```elisp
//! (define-ffi-function fff--grep-match-line-content
//!   "fff_grep_match_get_line_content" :pointer [:pointer] fff--library)
//!
//! (ffi-get-c-string (fff--grep-match-line-content match-ptr))
//! ```
//!
//! # Array iteration
//!
//! To walk result arrays use `fff_search_result_get_item`,
//! `fff_grep_result_get_match`, and `fff_search_result_get_score` — these are
//! defined in the main `lib.rs` FFI surface alongside the search functions.

use std::ffi::c_char;
use std::ptr;

use crate::ffi_types::{FffFileItem, FffGrepMatch, FffGrepResult, FffMatchRange, FffSearchResult};

// ── FffFileItem ──────────────────────────────────────────────────────────────

/// Returns the relative path of a file item (e.g. `"src/main.rs"`).
///
/// Returns null if `item` is null. The returned pointer is valid for the
/// lifetime of the owning `FffSearchResult`; do not free it directly.
///
/// ## Safety
/// `item` must be a valid `FffFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_file_item_get_relative_path(
    item: *const FffFileItem,
) -> *const c_char {
    if item.is_null() {
        return ptr::null();
    }
    unsafe { (*item).relative_path }
}

/// Returns the file-name component of a file item (e.g. `"main.rs"`).
///
/// Returns null if `item` is null. Do not free the returned pointer.
///
/// ## Safety
/// `item` must be a valid `FffFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_file_item_get_file_name(
    item: *const FffFileItem,
) -> *const c_char {
    if item.is_null() {
        return ptr::null();
    }
    unsafe { (*item).file_name }
}

/// Returns the git status string for a file item (e.g. `"M "`, `"??"`)
/// or null if git is unavailable, the file is untracked, or `item` is null.
///
/// Do not free the returned pointer.
///
/// ## Safety
/// `item` must be a valid `FffFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_file_item_get_git_status(
    item: *const FffFileItem,
) -> *const c_char {
    if item.is_null() {
        return ptr::null();
    }
    unsafe { (*item).git_status }
}

/// Returns the file size in bytes. Returns `0` if `item` is null.
///
/// ## Safety
/// `item` must be a valid `FffFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_file_item_get_size(item: *const FffFileItem) -> u64 {
    if item.is_null() {
        return 0;
    }
    unsafe { (*item).size }
}

/// Returns the last-modified time as seconds since the UNIX epoch.
/// Returns `0` if `item` is null.
///
/// ## Safety
/// `item` must be a valid `FffFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_file_item_get_modified(item: *const FffFileItem) -> u64 {
    if item.is_null() {
        return 0;
    }
    unsafe { (*item).modified }
}

/// Returns the combined frecency score. Returns `0` if `item` is null.
///
/// ## Safety
/// `item` must be a valid `FffFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_file_item_get_total_frecency_score(
    item: *const FffFileItem,
) -> i64 {
    if item.is_null() {
        return 0;
    }
    unsafe { (*item).total_frecency_score }
}

/// Returns the access-based frecency score. Returns `0` if `item` is null.
///
/// ## Safety
/// `item` must be a valid `FffFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_file_item_get_access_frecency_score(
    item: *const FffFileItem,
) -> i64 {
    if item.is_null() {
        return 0;
    }
    unsafe { (*item).access_frecency_score }
}

/// Returns the modification-based frecency score. Returns `0` if `item` is null.
///
/// ## Safety
/// `item` must be a valid `FffFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_file_item_get_modification_frecency_score(
    item: *const FffFileItem,
) -> i64 {
    if item.is_null() {
        return 0;
    }
    unsafe { (*item).modification_frecency_score }
}

/// Returns `true` if the file was detected as binary. Returns `false` if `item` is null.
///
/// ## Safety
/// `item` must be a valid `FffFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_file_item_get_is_binary(item: *const FffFileItem) -> bool {
    if item.is_null() {
        return false;
    }
    unsafe { (*item).is_binary }
}

// ── FffGrepMatch ─────────────────────────────────────────────────────────────

/// Returns the relative path of the file containing this grep match.
///
/// Returns null if `m` is null. Do not free the returned pointer.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_relative_path(
    m: *const FffGrepMatch,
) -> *const c_char {
    if m.is_null() {
        return ptr::null();
    }
    unsafe { (*m).relative_path }
}

/// Returns the file-name component of the file containing this grep match.
///
/// Returns null if `m` is null. Do not free the returned pointer.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_file_name(
    m: *const FffGrepMatch,
) -> *const c_char {
    if m.is_null() {
        return ptr::null();
    }
    unsafe { (*m).file_name }
}

/// Returns the git status string for the matched file (e.g. `"M "`, `"??"`)
/// or null if git is unavailable, the file is untracked, or `m` is null.
///
/// Do not free the returned pointer.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_git_status(
    m: *const FffGrepMatch,
) -> *const c_char {
    if m.is_null() {
        return ptr::null();
    }
    unsafe { (*m).git_status }
}

/// Returns the full text content of the matched line.
///
/// Returns null if `m` is null. Do not free the returned pointer.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_line_content(
    m: *const FffGrepMatch,
) -> *const c_char {
    if m.is_null() {
        return ptr::null();
    }
    unsafe { (*m).line_content }
}

/// Returns the 1-based line number of the match within its file.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_line_number(m: *const FffGrepMatch) -> u64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).line_number }
}

/// Returns the 0-based column of the match start within its line.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_col(m: *const FffGrepMatch) -> u32 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).col }
}

/// Returns the byte offset of the match start from the beginning of the file.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_byte_offset(m: *const FffGrepMatch) -> u64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).byte_offset }
}

/// Returns the file size in bytes for the matched file. Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_size(m: *const FffGrepMatch) -> u64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).size }
}

/// Returns the combined frecency score for the matched file.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_total_frecency_score(
    m: *const FffGrepMatch,
) -> i64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).total_frecency_score }
}

/// Returns the access-based frecency score for the matched file.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_access_frecency_score(
    m: *const FffGrepMatch,
) -> i64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).access_frecency_score }
}

/// Returns the modification-based frecency score for the matched file.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_modification_frecency_score(
    m: *const FffGrepMatch,
) -> i64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).modification_frecency_score }
}

/// Returns the last-modified time as seconds since the UNIX epoch for the matched file.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_modified(m: *const FffGrepMatch) -> u64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).modified }
}

/// Returns the number of highlight ranges in this match. Returns `0` if `m` is null.
///
/// Use with [`fff_grep_match_get_match_range`] to iterate the highlight spans.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_match_ranges_count(m: *const FffGrepMatch) -> u32 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).match_ranges_count }
}

/// Returns a pointer to the `index`-th [`FffMatchRange`] highlight span.
///
/// Returns null if `m` is null, `index >= match_ranges_count`, or the
/// ranges array is null. The returned pointer is valid until the owning
/// `FffGrepResult` is freed; do not free it directly.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_match_range(
    m: *const FffGrepMatch,
    index: u32,
) -> *const FffMatchRange {
    if m.is_null() {
        return ptr::null();
    }
    let m = unsafe { &*m };
    if index >= m.match_ranges_count || m.match_ranges.is_null() {
        return ptr::null();
    }
    unsafe { m.match_ranges.add(index as usize) }
}

/// Returns the number of context lines captured before the match.
/// Returns `0` if `m` is null.
///
/// Use with [`fff_grep_match_get_context_before`] to read each line.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_context_before_count(m: *const FffGrepMatch) -> u32 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).context_before_count }
}

/// Returns the `index`-th context line before the match.
///
/// Returns null if `m` is null, `index >= context_before_count`, or the
/// context array is null. Do not free the returned pointer.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_context_before(
    m: *const FffGrepMatch,
    index: u32,
) -> *const c_char {
    if m.is_null() {
        return ptr::null();
    }
    let m = unsafe { &*m };
    if index >= m.context_before_count || m.context_before.is_null() {
        return ptr::null();
    }
    unsafe { *m.context_before.add(index as usize) }
}

/// Returns the number of context lines captured after the match.
/// Returns `0` if `m` is null.
///
/// Use with [`fff_grep_match_get_context_after`] to read each line.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_context_after_count(m: *const FffGrepMatch) -> u32 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).context_after_count }
}

/// Returns the `index`-th context line after the match.
///
/// Returns null if `m` is null, `index >= context_after_count`, or the
/// context array is null. Do not free the returned pointer.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_context_after(
    m: *const FffGrepMatch,
    index: u32,
) -> *const c_char {
    if m.is_null() {
        return ptr::null();
    }
    let m = unsafe { &*m };
    if index >= m.context_after_count || m.context_after.is_null() {
        return ptr::null();
    }
    unsafe { *m.context_after.add(index as usize) }
}

/// Returns the fuzzy match score. Returns `0` if `m` is null or no fuzzy
/// score is present.
///
/// Always check [`fff_grep_match_get_has_fuzzy_score`] first; `0` is
/// ambiguous without that flag.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_fuzzy_score(m: *const FffGrepMatch) -> u16 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).fuzzy_score }
}

/// Returns `true` if this match carries a valid fuzzy score.
/// Returns `false` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_has_fuzzy_score(m: *const FffGrepMatch) -> bool {
    if m.is_null() {
        return false;
    }
    unsafe { (*m).has_fuzzy_score }
}

/// Returns `true` if the match was identified as a symbol definition.
/// Returns `false` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_is_definition(m: *const FffGrepMatch) -> bool {
    if m.is_null() {
        return false;
    }
    unsafe { (*m).is_definition }
}

/// Returns `true` if the matched file was detected as binary.
/// Returns `false` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FffGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_match_get_is_binary(m: *const FffGrepMatch) -> bool {
    if m.is_null() {
        return false;
    }
    unsafe { (*m).is_binary }
}

// ── FffSearchResult ──────────────────────────────────────────────────────────

/// Returns the number of items in the result. Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FffSearchResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_search_result_get_count(r: *const FffSearchResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).count }
}

/// Returns the total number of files that matched before the result was
/// truncated to the page size. Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FffSearchResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_search_result_get_total_matched(r: *const FffSearchResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).total_matched }
}

/// Returns the total number of indexed files considered during search.
/// Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FffSearchResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_search_result_get_total_files(r: *const FffSearchResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).total_files }
}

// ── FffGrepResult ─────────────────────────────────────────────────────────────

/// Returns the number of matches in the result. Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FffGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_result_get_count(r: *const FffGrepResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).count }
}

/// Returns the total number of matches found across all pages.
/// Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FffGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_result_get_total_matched(r: *const FffGrepResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).total_matched }
}

/// Returns the number of files actually opened and searched in this call.
/// Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FffGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_result_get_total_files_searched(r: *const FffGrepResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).total_files_searched }
}

/// Returns the total number of indexed files before any filtering.
/// Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FffGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_result_get_total_files(r: *const FffGrepResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).total_files }
}

/// Returns the number of files eligible for search after path/type filtering.
/// Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FffGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_result_get_filtered_file_count(r: *const FffGrepResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).filtered_file_count }
}

/// Returns the file offset for the next page, or `0` if all files have been
/// searched or `r` is null. Pass this value as `file_offset` to a subsequent
/// `fff_live_grep` or `fff_multi_grep` call to continue pagination.
///
/// ## Safety
/// `r` must be a valid `FffGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_result_get_next_file_offset(r: *const FffGrepResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).next_file_offset }
}

/// Returns the regex compilation error string if the engine fell back to
/// literal matching, or null if there was no error or `r` is null.
///
/// Do not free the returned pointer.
///
/// ## Safety
/// `r` must be a valid `FffGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_result_get_regex_fallback_error(
    r: *const FffGrepResult,
) -> *const c_char {
    if r.is_null() {
        return ptr::null();
    }
    unsafe { (*r).regex_fallback_error }
}
