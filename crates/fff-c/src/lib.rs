//! C FFI bindings for fff-core
//!
//! This crate provides C-compatible FFI exports that can be used from any language
//! with C FFI support (Bun, Node.js, Python, Ruby, etc.).
//!
//! # Instance-based API
//!
//! All state is owned by an opaque `FffInstance` fff_handle. Callers create an instance
//! with `fff_create_instance`, pass the fff_handle to every subsequent call, and free it with
//! `fff_destroy`. Multiple independent instances can coexist in the same process.
//!
//! # Memory management
//!
//! * Every `fff_*` function that returns `*mut FffResult` requires the caller to
//!   free the result with `fff_free_result`.
//! * The instance itself must be freed with `fff_destroy`.
//!
//! # Parameter conventions
//!
//! * Optional `*const c_char` parameters: pass NULL or an empty string to omit.
//! * Numeric parameters: 0 means "use default" unless documented otherwise.
//! * Grep mode (`u8`): 0 = plain text, 1 = regex, 2 = fuzzy.
//! * Multi-grep patterns are passed as a single newline-separated (`\n`) string.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::PathBuf;
use std::time::Duration;

use fff::shared::SharedQueryTracker;

mod ffi_types;

use fff::file_picker::FilePicker;
use fff::frecency::FrecencyTracker;
use fff::query_tracker::QueryTracker;
use fff::{DbHealthChecker, FFFMode, FuzzySearchOptions, PaginationArgs, QueryParser};
use fff::{SharedFrecency, SharedPicker};
use ffi_types::{
    FffFileItem, FffGrepMatch, FffGrepResult, FffResult, FffScanProgress, FffScore, FffSearchResult,
};

/// Opaque fff_handle holding all per-instance state.
///
/// The caller receives this as `*mut c_void` and must pass it to every FFI call.
/// The fff_handle is freed by `fff_destroy`.
struct FffInstance {
    picker: SharedPicker,
    frecency: SharedFrecency,
    query_tracker: SharedQueryTracker,
}

/// Helper to convert C string to Rust &str.
///
/// Returns `None` if the pointer is null or the string is not valid UTF-8.
unsafe fn cstr_to_str<'a>(s: *const c_char) -> Option<&'a str> {
    if s.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(s).to_str().ok() }
    }
}

/// Helper to convert an optional C string parameter.
///
/// Returns `None` if the pointer is null, empty, or not valid UTF-8.
unsafe fn optional_cstr<'a>(s: *const c_char) -> Option<&'a str> {
    unsafe { cstr_to_str(s) }.filter(|s| !s.is_empty())
}

/// Recover a `&FffInstance` from the opaque pointer.
///
/// Returns an error `FffResult` if the pointer is null.
unsafe fn instance_ref<'a>(fff_handle: *mut c_void) -> Result<&'a FffInstance, *mut FffResult> {
    if fff_handle.is_null() {
        Err(FffResult::err(
            "Instance handle is null. Create one with fff_create_instance first.",
        ))
    } else {
        Ok(unsafe { &*(fff_handle as *const FffInstance) })
    }
}

/// Decode a `u8` grep mode into the core enum.
fn grep_mode_from_u8(mode: u8) -> fff::GrepMode {
    match mode {
        1 => fff::GrepMode::Regex,
        2 => fff::GrepMode::Fuzzy,
        _ => fff::GrepMode::PlainText,
    }
}

/// Apply "0 means default" convention.
fn default_u32(val: u32, default: u32) -> u32 {
    if val == 0 { default } else { val }
}

fn default_u64(val: u64, default: u64) -> u64 {
    if val == 0 { default } else { val }
}

fn default_i32(val: i32, default: i32) -> i32 {
    if val == 0 { default } else { val }
}

/// Create a new file finder instance.
///
/// Returns an opaque pointer that must be passed to all other `fff_*` calls
/// and eventually freed with `fff_destroy`.
///
/// # Parameters
///
/// * `base_path`          – directory to index (required)
/// * `frecency_db_path`   – path to frecency LMDB database (NULL/empty to skip)
/// * `history_db_path`    – path to query history LMDB database (NULL/empty to skip)
/// * `use_unsafe_no_lock` – use MDB_NOLOCK for LMDB (useful in single-process setups)
/// * `warmup_mmap_cache`  – pre-populate mmap caches after the initial scan
/// * `ai_mode`            – enable AI-agent optimizations (auto-track frecency on modifications)
///
/// ## Safety
/// String parameters must be valid null-terminated UTF-8 or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_create_instance(
    base_path: *const c_char,
    frecency_db_path: *const c_char,
    history_db_path: *const c_char,
    use_unsafe_no_lock: bool,
    warmup_mmap_cache: bool,
    ai_mode: bool,
) -> *mut FffResult {
    let base_path_str = match unsafe { cstr_to_str(base_path) } {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return FffResult::err("base_path is null or empty"),
    };

    let frecency_path = unsafe { optional_cstr(frecency_db_path) }.map(|s| s.to_string());
    let history_path = unsafe { optional_cstr(history_db_path) }.map(|s| s.to_string());

    // Create shared state that background threads will write into.
    let shared_picker = SharedPicker::default();
    let shared_frecency = SharedFrecency::default();
    let query_tracker = SharedQueryTracker::default();

    // Initialize frecency tracker if path is provided
    if let Some(ref frecency_path) = frecency_path {
        if let Some(parent) = PathBuf::from(frecency_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        match FrecencyTracker::new(frecency_path, use_unsafe_no_lock) {
            Ok(tracker) => {
                if let Err(e) = shared_frecency.init(tracker) {
                    return FffResult::err(&format!("Failed to acquire frecency lock: {}", e));
                }
                let _ = shared_frecency.spawn_gc(frecency_path.clone(), use_unsafe_no_lock);
            }
            Err(e) => return FffResult::err(&format!("Failed to init frecency db: {}", e)),
        }
    }

    // Initialize query tracker if path is provided
    if let Some(ref history_path) = history_path {
        if let Some(parent) = PathBuf::from(history_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        match QueryTracker::new(history_path, use_unsafe_no_lock) {
            Ok(tracker) => {
                if let Err(e) = query_tracker.init(tracker) {
                    return FffResult::err(&format!("Failed to acquire query tracker lock: {}", e));
                }
            }
            Err(e) => return FffResult::err(&format!("Failed to init query tracker db: {}", e)),
        }
    }

    let mode = if ai_mode {
        FFFMode::Ai
    } else {
        FFFMode::Neovim
    };

    // Initialize file picker (writes directly into shared_picker)
    if let Err(e) = FilePicker::new_with_shared_state(
        shared_picker.clone(),
        shared_frecency.clone(),
        fff::FilePickerOptions {
            base_path: base_path_str,
            warmup_mmap_cache,
            mode,
            cache_budget: None,
            ..Default::default()
        },
    ) {
        return FffResult::err(&format!("Failed to init file picker: {}", e));
    }

    let instance = Box::new(FffInstance {
        picker: shared_picker,
        frecency: shared_frecency,
        query_tracker,
    });

    let fff_handle = Box::into_raw(instance) as *mut c_void;
    FffResult::ok_handle(fff_handle)
}

/// Destroy a file finder instance and free all its resources.
///
/// ## Safety
/// `fff_handle` must be a valid pointer returned by `fff_create_instance`, or null (no-op).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_destroy(fff_handle: *mut c_void) {
    if fff_handle.is_null() {
        return;
    }

    let instance = unsafe { Box::from_raw(fff_handle as *mut FffInstance) };

    if let Ok(mut guard) = instance.picker.write()
        && let Some(mut picker) = guard.take()
    {
        picker.stop_background_monitor();
    }

    if let Ok(mut guard) = instance.frecency.write() {
        *guard = None;
    }
    if let Ok(mut guard) = instance.query_tracker.write() {
        *guard = None;
    }
}

/// Perform fuzzy search on indexed files.
///
/// # Parameters
///
/// * `fff_handle`              – instance from `fff_create_instance`
/// * `query`                   – search query string
/// * `current_file`            – path of the currently open file for deprioritization (NULL/empty to skip)
/// * `max_threads`             – maximum worker threads (0 = auto-detect)
/// * `page_index`              – pagination offset (0 = first page)
/// * `page_size`               – results per page (0 = default 100)
/// * `combo_boost_multiplier`  – score multiplier for combo matches (0 = default 100)
/// * `min_combo_count`         – minimum combo count before boost applies (0 = default 3)
///
/// ## Safety
/// * `fff_handle` must be a valid instance pointer from `fff_create_instance`.
/// * `query` and `current_file` must be valid null-terminated UTF-8 strings or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_search(
    fff_handle: *mut c_void,
    query: *const c_char,
    current_file: *const c_char,
    max_threads: u32,
    page_index: u32,
    page_size: u32,
    combo_boost_multiplier: i32,
    min_combo_count: u32,
) -> *mut FffResult {
    let inst = match unsafe { instance_ref(fff_handle) } {
        Ok(i) => i,
        Err(e) => return e,
    };

    let query_str = match unsafe { cstr_to_str(query) } {
        Some(s) => s,
        None => return FffResult::err("Query is null or invalid UTF-8"),
    };

    let current_file_str = unsafe { optional_cstr(current_file) };
    let page_size = default_u32(page_size, 100) as usize;
    let min_combo_count = default_u32(min_combo_count, 3);
    let combo_boost_multiplier = default_i32(combo_boost_multiplier, 100);

    let picker_guard = match inst.picker.read() {
        Ok(g) => g,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    let picker = match picker_guard.as_ref() {
        Some(p) => p,
        None => {
            return FffResult::err("File picker not initialized. Call fff_create_instance first.");
        }
    };

    // Get query tracker ref for combo matching
    let qt_guard = match inst.query_tracker.read() {
        Ok(q) => q,
        Err(_) => return FffResult::err("Failed to acquire query tracker lock"),
    };
    let query_tracker_ref = qt_guard.as_ref();

    let parser = QueryParser::default();
    let parsed = parser.parse(query_str);

    let results = FilePicker::fuzzy_search(
        picker.get_files(),
        &parsed,
        query_tracker_ref,
        FuzzySearchOptions {
            max_threads: max_threads as usize,
            current_file: current_file_str,
            project_path: Some(picker.base_path()),
            combo_boost_score_multiplier: combo_boost_multiplier,
            min_combo_count,
            pagination: PaginationArgs {
                offset: page_index as usize,
                limit: page_size,
            },
        },
    );

    let search_result = FffSearchResult::from_core(&results);
    FffResult::ok_handle(search_result as *mut c_void)
}

/// Perform content search (grep) across indexed files.
///
/// # Parameters
///
/// * `fff_handle`            – instance from `fff_create_instance`
/// * `query`                 – search query (supports constraint syntax like `*.rs pattern`)
/// * `mode`                  – 0 = plain text (SIMD), 1 = regex, 2 = fuzzy
/// * `max_file_size`         – skip files larger than this in bytes (0 = default 10 MB)
/// * `max_matches_per_file`  – max matches per file (0 = unlimited)
/// * `smart_case`            – case-insensitive when query is all lowercase
/// * `file_offset`           – file-based pagination offset (0 = start)
/// * `page_limit`            – max matches to return (0 = default 50)
/// * `time_budget_ms`        – wall-clock budget in ms (0 = unlimited)
/// * `before_context`        – context lines before each match
/// * `after_context`         – context lines after each match
/// * `classify_definitions`  – tag matches that are code definitions
///
/// ## Safety
/// * `fff_handle` must be a valid instance pointer from `fff_create_instance`.
/// * `query` must be a valid null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_live_grep(
    fff_handle: *mut c_void,
    query: *const c_char,
    mode: u8,
    max_file_size: u64,
    max_matches_per_file: u32,
    smart_case: bool,
    file_offset: u32,
    page_limit: u32,
    time_budget_ms: u64,
    before_context: u32,
    after_context: u32,
    classify_definitions: bool,
) -> *mut FffResult {
    let inst = match unsafe { instance_ref(fff_handle) } {
        Ok(i) => i,
        Err(e) => return e,
    };

    let query_str = match unsafe { cstr_to_str(query) } {
        Some(s) => s,
        None => return FffResult::err("Query is null or invalid UTF-8"),
    };

    let picker_guard = match inst.picker.read() {
        Ok(g) => g,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    let picker = match picker_guard.as_ref() {
        Some(p) => p,
        None => {
            return FffResult::err("File picker not initialized. Call fff_create_instance first.");
        }
    };

    let is_ai = picker.mode().is_ai();
    let parsed = if is_ai {
        fff::QueryParser::new(fff_query_parser::AiGrepConfig).parse(query_str)
    } else {
        fff::grep::parse_grep_query(query_str)
    };

    let options = fff::GrepSearchOptions {
        max_file_size: default_u64(max_file_size, 10 * 1024 * 1024),
        max_matches_per_file: max_matches_per_file as usize,
        smart_case,
        file_offset: file_offset as usize,
        page_limit: default_u32(page_limit, 50) as usize,
        mode: grep_mode_from_u8(mode),
        time_budget_ms,
        before_context: before_context as usize,
        after_context: after_context as usize,
        classify_definitions,
        trim_whitespace: false,
    };

    let result = picker.grep(&parsed, &options);
    let grep_result = FffGrepResult::from_core(&result);
    FffResult::ok_handle(grep_result as *mut c_void)
}

/// Perform multi-pattern OR search (Aho-Corasick) across indexed files.
///
/// Searches for lines matching ANY of the provided patterns using
/// SIMD-accelerated multi-needle matching.
///
/// # Parameters
///
/// * `fff_handle`              – instance from `fff_create_instance`
/// * `patterns_joined`         – patterns separated by `\n` (e.g. `"foo\nbar\nbaz"`)
/// * `constraints`             – file filter like `"*.rs"` or `"/src/"` (NULL/empty to skip)
/// * `max_file_size`           – skip files larger than this in bytes (0 = default 10 MB)
/// * `max_matches_per_file`    – max matches per file (0 = unlimited)
/// * `smart_case`              – case-insensitive when all patterns are lowercase
/// * `file_offset`             – file-based pagination offset (0 = start)
/// * `page_limit`              – max matches to return (0 = default 50)
/// * `time_budget_ms`          – wall-clock budget in ms (0 = unlimited)
/// * `before_context`          – context lines before each match
/// * `after_context`           – context lines after each match
/// * `classify_definitions`    – tag matches that are code definitions
///
/// ## Safety
/// * `fff_handle` must be a valid instance pointer from `fff_create_instance`.
/// * `patterns_joined` and `constraints` must be valid null-terminated UTF-8 or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_multi_grep(
    fff_handle: *mut c_void,
    patterns_joined: *const c_char,
    constraints: *const c_char,
    max_file_size: u64,
    max_matches_per_file: u32,
    smart_case: bool,
    file_offset: u32,
    page_limit: u32,
    time_budget_ms: u64,
    before_context: u32,
    after_context: u32,
    classify_definitions: bool,
) -> *mut FffResult {
    let inst = match unsafe { instance_ref(fff_handle) } {
        Ok(i) => i,
        Err(e) => return e,
    };

    let patterns_str = match unsafe { cstr_to_str(patterns_joined) } {
        Some(s) if !s.is_empty() => s,
        _ => return FffResult::err("patterns_joined is null or empty"),
    };

    let patterns: Vec<&str> = patterns_str.split('\n').collect();
    if patterns.is_empty() || patterns.iter().all(|p| p.is_empty()) {
        return FffResult::err("patterns must not be empty");
    }

    let constraints_str = unsafe { optional_cstr(constraints) };

    let picker_guard = match inst.picker.read() {
        Ok(g) => g,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    let picker = match picker_guard.as_ref() {
        Some(p) => p,
        None => {
            return FffResult::err("File picker not initialized. Call fff_create_instance first.");
        }
    };

    let is_ai = picker.mode().is_ai();

    // Parse constraints from the optional string (e.g. "*.rs /src/")
    let parsed_constraints = constraints_str.map(|c| {
        if is_ai {
            fff::QueryParser::new(fff_query_parser::AiGrepConfig).parse(c)
        } else {
            fff::grep::parse_grep_query(c)
        }
    });

    let constraint_refs: &[fff::Constraint<'_>] = match &parsed_constraints {
        Some(q) => &q.constraints,
        None => &[],
    };

    let options = fff::GrepSearchOptions {
        max_file_size: default_u64(max_file_size, 10 * 1024 * 1024),
        max_matches_per_file: max_matches_per_file as usize,
        smart_case,
        file_offset: file_offset as usize,
        page_limit: default_u32(page_limit, 50) as usize,
        mode: fff::GrepMode::PlainText, // ignored by multi_grep_search
        time_budget_ms,
        before_context: before_context as usize,
        after_context: after_context as usize,
        classify_definitions,
        trim_whitespace: false,
    };

    let overlay_guard = picker.bigram_overlay().map(|o| o.read());
    let result = fff::multi_grep_search(
        picker.get_files(),
        &patterns,
        constraint_refs,
        &options,
        picker.cache_budget(),
        picker.bigram_index(),
        overlay_guard.as_deref(),
        None,
    );
    let grep_result = FffGrepResult::from_core(&result);
    FffResult::ok_handle(grep_result as *mut c_void)
}

/// Trigger a rescan of the file index.
///
/// ## Safety
/// `fff_handle` must be a valid instance pointer from `fff_create_instance`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_scan_files(fff_handle: *mut c_void) -> *mut FffResult {
    let inst = match unsafe { instance_ref(fff_handle) } {
        Ok(i) => i,
        Err(e) => return e,
    };

    let mut guard = match inst.picker.write() {
        Ok(g) => g,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    let picker = match guard.as_mut() {
        Some(p) => p,
        None => return FffResult::err("File picker not initialized"),
    };

    match picker.trigger_rescan(&inst.frecency) {
        Ok(_) => FffResult::ok_empty(),
        Err(e) => FffResult::err(&format!("Failed to trigger rescan: {}", e)),
    }
}

/// Check if a scan is currently in progress.
///
/// ## Safety
/// `fff_handle` must be a valid instance pointer from `fff_create_instance`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_is_scanning(fff_handle: *mut c_void) -> bool {
    let inst = match unsafe { instance_ref(fff_handle) } {
        Ok(i) => i,
        Err(_) => return false,
    };

    inst.picker
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().map(|p| p.is_scan_active()))
        .unwrap_or(false)
}

/// Get scan progress information.
///
/// ## Safety
/// `fff_handle` must be a valid instance pointer from `fff_create_instance`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_get_scan_progress(fff_handle: *mut c_void) -> *mut FffResult {
    let inst = match unsafe { instance_ref(fff_handle) } {
        Ok(i) => i,
        Err(e) => return e,
    };

    let guard = match inst.picker.read() {
        Ok(g) => g,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    let picker = match guard.as_ref() {
        Some(p) => p,
        None => return FffResult::err("File picker not initialized"),
    };

    let result = Box::into_raw(Box::new(FffScanProgress::from(picker.get_scan_progress())));
    FffResult::ok_handle(result as *mut c_void)
}

/// Wait for initial scan to complete.
///
/// ## Safety
/// `fff_handle` must be a valid instance pointer from `fff_create_instance`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_wait_for_scan(
    fff_handle: *mut c_void,
    timeout_ms: u64,
) -> *mut FffResult {
    let FffInstance { picker, .. } = match unsafe { instance_ref(fff_handle) } {
        Ok(i) => i,
        Err(e) => return e,
    };

    let completed = picker.wait_for_scan(Duration::from_millis(timeout_ms));
    FffResult::ok_int(completed as i64)
}

/// Wait for the background file watcher to be ready.
///
/// ## Safety
/// `fff_handle` must be a valid instance pointer from `fff_create_instance`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_wait_for_watcher(
    fff_handle: *mut c_void,
    timeout_ms: u64,
) -> *mut FffResult {
    let inst = match unsafe { instance_ref(fff_handle) } {
        Ok(i) => i,
        Err(e) => return e,
    };

    let completed = inst
        .picker
        .wait_for_watcher(Duration::from_millis(timeout_ms));
    FffResult::ok_int(completed as i64)
}

/// Restart indexing in a new directory.
///
/// ## Safety
/// * `fff_handle` must be a valid instance pointer from `fff_create_instance`.
/// * `new_path` must be a valid null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_restart_index(
    fff_handle: *mut c_void,
    new_path: *const c_char,
) -> *mut FffResult {
    let inst = match unsafe { instance_ref(fff_handle) } {
        Ok(i) => i,
        Err(e) => return e,
    };

    let path_str = match unsafe { cstr_to_str(new_path) } {
        Some(s) => s,
        None => return FffResult::err("Path is null or invalid UTF-8"),
    };

    let path = PathBuf::from(&path_str);
    if !path.exists() {
        return FffResult::err(&format!("Path does not exist: {}", path_str));
    }

    let canonical_path = match fff::path_utils::canonicalize(&path) {
        Ok(p) => p,
        Err(e) => return FffResult::err(&format!("Failed to canonicalize path: {}", e)),
    };

    let mut guard = match inst.picker.write() {
        Ok(g) => g,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    let (warmup_caches, mode) = if let Some(mut picker) = guard.take() {
        let warmup = picker.need_warmup_mmap_cache();
        let mode = picker.mode();
        picker.stop_background_monitor();
        (warmup, mode)
    } else {
        (false, FFFMode::default())
    };

    drop(guard);

    match FilePicker::new_with_shared_state(
        inst.picker.clone(),
        inst.frecency.clone(),
        fff::FilePickerOptions {
            base_path: canonical_path.to_string_lossy().to_string(),
            warmup_mmap_cache: warmup_caches,
            mode,
            cache_budget: None,
            ..Default::default()
        },
    ) {
        Ok(()) => FffResult::ok_empty(),
        Err(e) => FffResult::err(&format!("Failed to init file picker: {}", e)),
    }
}

/// Refresh git status cache.
///
/// ## Safety
/// `fff_handle` must be a valid instance pointer from `fff_create_instance`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_refresh_git_status(fff_handle: *mut c_void) -> *mut FffResult {
    let inst = match unsafe { instance_ref(fff_handle) } {
        Ok(i) => i,
        Err(e) => return e,
    };

    match inst.picker.refresh_git_status(&inst.frecency) {
        Ok(count) => FffResult::ok_int(count as i64),
        Err(e) => FffResult::err(&format!("Failed to refresh git status: {}", e)),
    }
}

/// Track query completion for smart suggestions.
///
/// ## Safety
/// * `fff_handle` must be a valid instance pointer from `fff_create_instance`.
/// * `query` and `file_path` must be valid null-terminated UTF-8 strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_track_query(
    fff_handle: *mut c_void,
    query: *const c_char,
    file_path: *const c_char,
) -> *mut FffResult {
    let inst = match unsafe { instance_ref(fff_handle) } {
        Ok(i) => i,
        Err(e) => return e,
    };

    let query_str = match unsafe { cstr_to_str(query) } {
        Some(s) => s,
        None => return FffResult::err("Query is null or invalid UTF-8"),
    };

    let path_str = match unsafe { cstr_to_str(file_path) } {
        Some(s) => s,
        None => return FffResult::err("File path is null or invalid UTF-8"),
    };

    let file_path = match fff::path_utils::canonicalize(path_str) {
        Ok(p) => p,
        Err(e) => return FffResult::err(&format!("Failed to canonicalize path: {}", e)),
    };

    let project_path = {
        let guard = match inst.picker.read() {
            Ok(g) => g,
            Err(_) => return FffResult::ok_int(0),
        };
        match guard.as_ref() {
            Some(p) => p.base_path().to_path_buf(),
            None => return FffResult::ok_int(0),
        }
    };

    let mut qt_guard = match inst.query_tracker.write() {
        Ok(q) => q,
        Err(_) => return FffResult::ok_int(0),
    };

    if let Some(ref mut tracker) = *qt_guard
        && let Err(e) = tracker.track_query_completion(query_str, &project_path, &file_path)
    {
        return FffResult::err(&format!("Failed to track query: {}", e));
    }

    FffResult::ok_int(1)
}

/// Get historical query by offset (0 = most recent).
///
/// ## Safety
/// `fff_handle` must be a valid instance pointer from `fff_create_instance`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_get_historical_query(
    fff_handle: *mut c_void,
    offset: u64,
) -> *mut FffResult {
    let inst = match unsafe { instance_ref(fff_handle) } {
        Ok(i) => i,
        Err(e) => return e,
    };

    let project_path = {
        let guard = match inst.picker.read() {
            Ok(g) => g,
            Err(_) => return FffResult::ok_empty(),
        };
        match guard.as_ref() {
            Some(p) => p.base_path().to_path_buf(),
            None => return FffResult::ok_empty(),
        }
    };

    let qt_guard = match inst.query_tracker.read() {
        Ok(q) => q,
        Err(_) => return FffResult::ok_empty(),
    };

    let tracker = match qt_guard.as_ref() {
        Some(t) => t,
        None => return FffResult::ok_empty(),
    };

    match tracker.get_historical_query(&project_path, offset as usize) {
        Ok(Some(query)) => FffResult::ok_string(&query),
        Ok(None) => FffResult::ok_empty(),
        Err(e) => FffResult::err(&format!("Failed to get historical query: {}", e)),
    }
}

/// Get health check information.
///
/// ## Safety
/// * `fff_handle` must be a valid instance pointer from `fff_create_instance`, or null for
///   a limited health check (version + git only).
/// * `test_path` can be null or a valid null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_health_check(
    fff_handle: *mut c_void,
    test_path: *const c_char,
) -> *mut FffResult {
    let test_path = unsafe { optional_cstr(test_path) }
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let mut health = serde_json::Map::new();
    health.insert(
        "version".to_string(),
        serde_json::Value::String(env!("CARGO_PKG_VERSION").to_string()),
    );

    // Git info
    let mut git_info = serde_json::Map::new();
    let git_version = git2::Version::get();
    let (major, minor, rev) = git_version.libgit2_version();
    git_info.insert(
        "libgit2_version".to_string(),
        serde_json::Value::String(format!("{}.{}.{}", major, minor, rev)),
    );

    match git2::Repository::discover(&test_path) {
        Ok(repo) => {
            git_info.insert("available".to_string(), serde_json::Value::Bool(true));
            git_info.insert(
                "repository_found".to_string(),
                serde_json::Value::Bool(true),
            );
            if let Some(workdir) = repo.workdir() {
                git_info.insert(
                    "workdir".to_string(),
                    serde_json::Value::String(workdir.to_string_lossy().to_string()),
                );
            }
        }
        Err(e) => {
            git_info.insert("available".to_string(), serde_json::Value::Bool(true));
            git_info.insert(
                "repository_found".to_string(),
                serde_json::Value::Bool(false),
            );
            git_info.insert(
                "error".to_string(),
                serde_json::Value::String(e.message().to_string()),
            );
        }
    }
    health.insert("git".to_string(), serde_json::Value::Object(git_info));

    let inst: Option<&FffInstance> = if fff_handle.is_null() {
        None
    } else {
        Some(unsafe { &*(fff_handle as *const FffInstance) })
    };

    // File picker info
    let mut picker_info = serde_json::Map::new();
    if let Some(inst) = inst {
        match inst.picker.read() {
            Ok(guard) => {
                if let Some(ref picker) = *guard {
                    picker_info.insert("initialized".to_string(), serde_json::Value::Bool(true));
                    picker_info.insert(
                        "base_path".to_string(),
                        serde_json::Value::String(picker.base_path().to_string_lossy().to_string()),
                    );
                    picker_info.insert(
                        "is_scanning".to_string(),
                        serde_json::Value::Bool(picker.is_scan_active()),
                    );
                    let progress = picker.get_scan_progress();
                    picker_info.insert(
                        "indexed_files".to_string(),
                        serde_json::Value::Number(progress.scanned_files_count.into()),
                    );
                } else {
                    picker_info.insert("initialized".to_string(), serde_json::Value::Bool(false));
                }
            }
            Err(_) => {
                picker_info.insert("initialized".to_string(), serde_json::Value::Bool(false));
                picker_info.insert(
                    "error".to_string(),
                    serde_json::Value::String("Failed to acquire lock".to_string()),
                );
            }
        }
    } else {
        picker_info.insert("initialized".to_string(), serde_json::Value::Bool(false));
    }
    health.insert(
        "file_picker".to_string(),
        serde_json::Value::Object(picker_info),
    );

    // Frecency info
    let mut frecency_info = serde_json::Map::new();
    if let Some(inst) = inst {
        match inst.frecency.read() {
            Ok(guard) => {
                frecency_info.insert(
                    "initialized".to_string(),
                    serde_json::Value::Bool(guard.is_some()),
                );
                if let Some(ref frecency) = *guard
                    && let Ok(health_data) = frecency.get_health()
                {
                    let mut db_health = serde_json::Map::new();
                    db_health.insert(
                        "path".to_string(),
                        serde_json::Value::String(health_data.path),
                    );
                    db_health.insert(
                        "disk_size".to_string(),
                        serde_json::Value::Number(health_data.disk_size.into()),
                    );
                    frecency_info.insert(
                        "db_healthcheck".to_string(),
                        serde_json::Value::Object(db_health),
                    );
                }
            }
            Err(_) => {
                frecency_info.insert("initialized".to_string(), serde_json::Value::Bool(false));
            }
        }
    } else {
        frecency_info.insert("initialized".to_string(), serde_json::Value::Bool(false));
    }
    health.insert(
        "frecency".to_string(),
        serde_json::Value::Object(frecency_info),
    );

    // Query tracker info
    let mut query_info = serde_json::Map::new();
    if let Some(inst) = inst {
        match inst.query_tracker.read() {
            Ok(guard) => {
                query_info.insert(
                    "initialized".to_string(),
                    serde_json::Value::Bool(guard.is_some()),
                );
                if let Some(ref tracker) = *guard
                    && let Ok(health_data) = tracker.get_health()
                {
                    let mut db_health = serde_json::Map::new();
                    db_health.insert(
                        "path".to_string(),
                        serde_json::Value::String(health_data.path),
                    );
                    db_health.insert(
                        "disk_size".to_string(),
                        serde_json::Value::Number(health_data.disk_size.into()),
                    );
                    query_info.insert(
                        "db_healthcheck".to_string(),
                        serde_json::Value::Object(db_health),
                    );
                }
            }
            Err(_) => {
                query_info.insert("initialized".to_string(), serde_json::Value::Bool(false));
            }
        }
    } else {
        query_info.insert("initialized".to_string(), serde_json::Value::Bool(false));
    }
    health.insert(
        "query_tracker".to_string(),
        serde_json::Value::Object(query_info),
    );

    match serde_json::to_string(&health) {
        Ok(json) => FffResult::ok_string(&json),
        Err(e) => FffResult::err(&format!("Failed to serialize health check: {}", e)),
    }
}

/// Free a search result returned by `fff_search`.
///
/// This frees the `FffSearchResult` struct, its `items` and `scores` arrays,
/// and all heap-allocated strings within each item and score.
///
/// ## Safety
/// `result` must be a valid pointer previously returned via `FffResult.handle`
/// from `fff_search`, or null (no-op).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_free_search_result(result: *mut FffSearchResult) {
    if result.is_null() {
        return;
    }

    unsafe {
        let result = Box::from_raw(result);
        let count = result.count as usize;

        if !result.items.is_null() {
            let mut items = Vec::from_raw_parts(result.items, count, count);
            for item in &mut items {
                item.free_strings();
            }
        }
        if !result.scores.is_null() {
            let mut scores = Vec::from_raw_parts(result.scores, count, count);
            for score in &mut scores {
                score.free_strings();
            }
        }
    }
}

/// Get a pointer to the `index`-th `FffFileItem` in a search result.
///
/// Returns null if `result` is null or `index >= result->count`.
/// The returned pointer is valid until the search result is freed.
///
/// ## Safety
/// `result` must be a valid `FffSearchResult` pointer from `fff_search`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_search_result_get_item(
    result: *const FffSearchResult,
    index: u32,
) -> *const FffFileItem {
    if result.is_null() {
        return std::ptr::null();
    }
    let result = unsafe { &*result };
    if index >= result.count || result.items.is_null() {
        return std::ptr::null();
    }
    unsafe { result.items.add(index as usize) }
}

/// Get a pointer to the `index`-th `FffScore` in a search result.
///
/// Returns null if `result` is null or `index >= result->count`.
/// The returned pointer is valid until the search result is freed.
///
/// ## Safety
/// `result` must be a valid `FffSearchResult` pointer from `fff_search`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_search_result_get_score(
    result: *const FffSearchResult,
    index: u32,
) -> *const FffScore {
    if result.is_null() {
        return std::ptr::null();
    }
    let result = unsafe { &*result };
    if index >= result.count || result.scores.is_null() {
        return std::ptr::null();
    }
    unsafe { result.scores.add(index as usize) }
}

/// Free a grep result returned by `fff_live_grep` or `fff_multi_grep`.
///
/// This frees the `FffGrepResult` struct, its `items` array, and all
/// heap-allocated strings, match ranges, and context arrays within each match.
///
/// ## Safety
/// `result` must be a valid pointer previously returned via `FffResult.handle`
/// from `fff_live_grep` or `fff_multi_grep`, or null (no-op).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_free_grep_result(result: *mut FffGrepResult) {
    if result.is_null() {
        return;
    }

    unsafe {
        let result = Box::from_raw(result);
        let count = result.count as usize;

        if !result.items.is_null() {
            let mut items = Vec::from_raw_parts(result.items, count, count);
            for item in &mut items {
                item.free_fields();
            }
        }
        if !result.regex_fallback_error.is_null() {
            drop(CString::from_raw(result.regex_fallback_error));
        }
    }
}

/// Get a pointer to the `index`-th `FffGrepMatch` in a grep result.
///
/// Returns null if `result` is null or `index >= result->count`.
/// The returned pointer is valid until the grep result is freed.
///
/// ## Safety
/// `result` must be a valid `FffGrepResult` pointer from `fff_live_grep` or `fff_multi_grep`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_grep_result_get_match(
    result: *const FffGrepResult,
    index: u32,
) -> *const FffGrepMatch {
    if result.is_null() {
        return std::ptr::null();
    }
    let result = unsafe { &*result };
    if index >= result.count || result.items.is_null() {
        return std::ptr::null();
    }
    unsafe { result.items.add(index as usize) }
}

/// Free a scan progress result returned by `fff_get_scan_progress`.
///
/// ## Safety
/// `result` must be a valid pointer previously returned via `FffResult.handle`
/// from `fff_get_scan_progress`, or null (no-op).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_free_scan_progress(result: *mut FffScanProgress) {
    if !result.is_null() {
        unsafe { drop(Box::from_raw(result)) };
    }
}

/// Offset a pointer by `byte_offset` bytes.
///
/// General-purpose utility for FFI consumers that need pointer arithmetic
/// (e.g. iterating over arrays). Returns null if `base` is null.
///
/// ## Safety
/// The resulting pointer must be within the bounds of the original allocation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_ptr_offset(base: *const c_void, byte_offset: usize) -> *const c_void {
    if base.is_null() {
        return std::ptr::null();
    }
    unsafe { (base as *const u8).add(byte_offset) as *const c_void }
}

/// Free a result returned by any `fff_*` function.
///
/// ## Safety
/// `result_ptr` must be a valid pointer returned by a `fff_*` function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_free_result(result_ptr: *mut FffResult) {
    if result_ptr.is_null() {
        return;
    }

    unsafe {
        let result = Box::from_raw(result_ptr);
        if !result.error.is_null() {
            drop(CString::from_raw(result.error));
        }
        // Note: `handle` is NOT freed here — the caller must free it
        // with the appropriate function (fff_destroy, fff_free_search_result,
        // fff_free_grep_result, fff_free_string, fff_free_scan_progress, etc.).
    }
}

/// Free a string returned by `fff_*` functions.
///
/// ## Safety
/// `s` must be a valid C string allocated by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_free_string(s: *mut c_char) {
    unsafe {
        if !s.is_null() {
            drop(CString::from_raw(s));
        }
    }
}
