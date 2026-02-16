//! C FFI bindings for fff-core
//!
//! This crate provides C-compatible FFI exports that can be used from any language
//! with C FFI support (Bun, Node.js, Python, Ruby, etc.).
//!
//! All functions return a pointer to a heap-allocated `FffResult` struct containing
//! success status and either data (as JSON string) or an error message.
//! Memory must be freed using `fff_free_result`.

use std::ffi::{CStr, CString, c_char};
use std::path::PathBuf;
use std::time::Duration;

mod ffi_types;

use fff_core::file_picker::FilePicker;
use fff_core::frecency::FrecencyTracker;
use fff_core::query_tracker::QueryTracker;
use fff_core::{DbHealthChecker, FuzzySearchOptions, PaginationArgs, QueryParser};
use fff_core::{FILE_PICKER, FRECENCY, QUERY_TRACKER};
use ffi_types::{FffResult, GrepSearchOptionsJson, InitOptions, ScanProgress, SearchOptions};
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Helper to convert C string to Rust &str.
///
/// Returns `None` if the pointer is null or the string is not valid UTF-8.
/// This is more efficient than `to_string_lossy()` as it returns a borrowed
/// `&str` directly without `Cow` overhead, and avoids replacement character
/// scanning since callers are expected to provide valid UTF-8.
unsafe fn cstr_to_str<'a>(s: *const c_char) -> Option<&'a str> {
    if s.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(s).to_str().ok() }
    }
}

/// Initialize the file finder with the given options (JSON string)
///
/// # Safety
/// `opts_json` must be a valid null-terminated UTF-8 string
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_init(opts_json: *const c_char) -> *mut FffResult {
    let opts_str = match unsafe { cstr_to_str(opts_json) } {
        Some(s) => s,
        None => return FffResult::err("Options JSON is null or invalid UTF-8"),
    };

    let opts: InitOptions = match serde_json::from_str(opts_str) {
        Ok(o) => o,
        Err(e) => return FffResult::err(&format!("Failed to parse options: {}", e)),
    };

    // Initialize frecency tracker if path is provided
    if let Some(frecency_path) = opts.frecency_db_path {
        // Ensure directory exists
        if let Some(parent) = PathBuf::from(&frecency_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut frecency = match FRECENCY.write() {
            Ok(f) => f,
            Err(e) => return FffResult::err(&format!("Failed to acquire frecency lock: {}", e)),
        };
        *frecency = None;
        match FrecencyTracker::new(&frecency_path, opts.use_unsafe_no_lock) {
            Ok(tracker) => *frecency = Some(tracker),
            Err(e) => return FffResult::err(&format!("Failed to init frecency db: {}", e)),
        }
        drop(frecency);
    }

    // Initialize query tracker if path is provided
    if let Some(history_path) = opts.history_db_path {
        // Ensure directory exists
        if let Some(parent) = PathBuf::from(&history_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut query_tracker = match QUERY_TRACKER.write() {
            Ok(q) => q,
            Err(e) => {
                return FffResult::err(&format!("Failed to acquire query tracker lock: {}", e));
            }
        };
        *query_tracker = None;
        match QueryTracker::new(&history_path, opts.use_unsafe_no_lock) {
            Ok(tracker) => *query_tracker = Some(tracker),
            Err(e) => return FffResult::err(&format!("Failed to init query tracker db: {}", e)),
        }
        drop(query_tracker);
    }

    // Initialize file picker
    let mut file_picker = match FILE_PICKER.write() {
        Ok(f) => f,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    if file_picker.is_some() {
        // Already initialized, clean up first
        if let Some(mut picker) = file_picker.take() {
            picker.stop_background_monitor();
        }
    }

    match FilePicker::with_options(opts.base_path, opts.warmup_mmap_cache) {
        Ok(picker) => {
            *file_picker = Some(picker);
            FffResult::ok_empty()
        }
        Err(e) => FffResult::err(&format!("Failed to init file picker: {}", e)),
    }
}

/// Destroy all resources and clean up
#[unsafe(no_mangle)]
pub extern "C" fn fff_destroy() -> *mut FffResult {
    // Clean up file picker
    if let Ok(mut file_picker) = FILE_PICKER.write()
        && let Some(mut picker) = file_picker.take()
    {
        picker.stop_background_monitor();
    }

    // Clean up frecency
    if let Ok(mut frecency) = FRECENCY.write() {
        *frecency = None;
    }

    // Clean up query tracker
    if let Ok(mut query_tracker) = QUERY_TRACKER.write() {
        *query_tracker = None;
    }

    FffResult::ok_empty()
}

// ============================================================================
// Search Functions
// ============================================================================

/// Perform fuzzy search on indexed files
///
/// # Safety
/// `query` and `opts_json` must be valid null-terminated UTF-8 strings
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_search(
    query: *const c_char,
    opts_json: *const c_char,
) -> *mut FffResult {
    let query_str = match unsafe { cstr_to_str(query) } {
        Some(s) => s,
        None => return FffResult::err("Query is null or invalid UTF-8"),
    };

    let opts: SearchOptions = if opts_json.is_null() {
        SearchOptions::default()
    } else {
        unsafe { cstr_to_str(opts_json) }
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    };

    let file_picker_guard = match FILE_PICKER.read() {
        Ok(f) => f,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    let picker = match file_picker_guard.as_ref() {
        Some(p) => p,
        None => return FffResult::err("File picker not initialized. Call fff_init first."),
    };

    let base_path = picker.base_path();
    let min_combo_count = opts.min_combo_count.unwrap_or(3);

    // Get last same query entry for combo matching
    let last_same_query_entry = {
        let query_tracker = match QUERY_TRACKER.read() {
            Ok(q) => q,
            Err(_) => return FffResult::err("Failed to acquire query tracker lock"),
        };

        query_tracker.as_ref().and_then(|tracker| {
            tracker
                .get_last_query_entry(query_str, base_path, min_combo_count)
                .ok()
                .flatten()
        })
    };

    // Parse the query
    let parser = QueryParser::default();
    let parsed = parser.parse(query_str);

    let results = FilePicker::fuzzy_search(
        picker.get_files(),
        query_str,
        parsed,
        FuzzySearchOptions {
            max_threads: opts.max_threads.unwrap_or(0),
            current_file: opts.current_file.as_deref(),
            project_path: Some(picker.base_path()),
            last_same_query_match: last_same_query_entry.as_ref(),
            combo_boost_score_multiplier: opts.combo_boost_multiplier.unwrap_or(100),
            min_combo_count,
            pagination: PaginationArgs {
                offset: opts.page_index.unwrap_or(0),
                limit: opts.page_size.unwrap_or(100),
            },
        },
    );

    // Convert to JSON
    let json_result = ffi_types::SearchResultJson::from_search_result(&results);
    match serde_json::to_string(&json_result) {
        Ok(json) => FffResult::ok_data(&json),
        Err(e) => FffResult::err(&format!("Failed to serialize results: {}", e)),
    }
}

/// Perform content search (grep) across indexed files
///
/// Searches file contents using the specified mode:
/// - "plain" (default): SIMD-accelerated literal text matching
/// - "regex": Regular expression matching
/// - "fuzzy": Smith-Waterman fuzzy matching per line
///
/// Results include file metadata and match locations with byte offsets
/// for highlighting. Supports file-based pagination via `file_offset`
/// and `next_file_offset` in the result.
///
/// # Safety
/// `query` and `opts_json` must be valid null-terminated UTF-8 strings
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_live_grep(
    query: *const c_char,
    opts_json: *const c_char,
) -> *mut FffResult {
    let query_str = match unsafe { cstr_to_str(query) } {
        Some(s) => s,
        None => return FffResult::err("Query is null or invalid UTF-8"),
    };

    let opts: GrepSearchOptionsJson = if opts_json.is_null() {
        GrepSearchOptionsJson::default()
    } else {
        unsafe { cstr_to_str(opts_json) }
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    };

    let file_picker_guard = match FILE_PICKER.read() {
        Ok(f) => f,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    let picker = match file_picker_guard.as_ref() {
        Some(p) => p,
        None => return FffResult::err("File picker not initialized. Call fff_init first."),
    };

    let mode = match opts.mode.as_deref() {
        Some("regex") => fff_core::GrepMode::Regex,
        Some("fuzzy") => fff_core::GrepMode::Fuzzy,
        _ => fff_core::GrepMode::PlainText,
    };

    let parsed = fff_core::grep::parse_grep_query(query_str);

    let options = fff_core::GrepSearchOptions {
        max_file_size: opts.max_file_size.unwrap_or(10 * 1024 * 1024),
        max_matches_per_file: opts.max_matches_per_file.unwrap_or(200),
        smart_case: opts.smart_case.unwrap_or(true),
        file_offset: opts.file_offset.unwrap_or(0),
        page_limit: opts.page_limit.unwrap_or(50),
        mode,
        time_budget_ms: opts.time_budget_ms.unwrap_or(0),
    };

    let result = fff_core::grep::grep_search(picker.get_files(), query_str, parsed, &options);

    let json_result = ffi_types::GrepResultJson::from_grep_result(&result);
    match serde_json::to_string(&json_result) {
        Ok(json) => FffResult::ok_data(&json),
        Err(e) => FffResult::err(&format!("Failed to serialize grep results: {}", e)),
    }
}

// ============================================================================
// File Index Functions
// ============================================================================

/// Trigger a rescan of the file index
#[unsafe(no_mangle)]
pub extern "C" fn fff_scan_files() -> *mut FffResult {
    let mut file_picker = match FILE_PICKER.write() {
        Ok(f) => f,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    let picker = match file_picker.as_mut() {
        Some(p) => p,
        None => return FffResult::err("File picker not initialized"),
    };

    match picker.trigger_rescan() {
        Ok(_) => FffResult::ok_empty(),
        Err(e) => FffResult::err(&format!("Failed to trigger rescan: {}", e)),
    }
}

/// Check if a scan is currently in progress
#[unsafe(no_mangle)]
pub extern "C" fn fff_is_scanning() -> bool {
    FILE_PICKER
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().map(|p| p.is_scan_active()))
        .unwrap_or(false)
}

/// Get scan progress information
#[unsafe(no_mangle)]
pub extern "C" fn fff_get_scan_progress() -> *mut FffResult {
    let file_picker = match FILE_PICKER.read() {
        Ok(f) => f,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    let picker = match file_picker.as_ref() {
        Some(p) => p,
        None => return FffResult::err("File picker not initialized"),
    };

    let progress = picker.get_scan_progress();
    let result = ScanProgress {
        scanned_files_count: progress.scanned_files_count,
        is_scanning: progress.is_scanning,
    };

    match serde_json::to_string(&result) {
        Ok(json) => FffResult::ok_data(&json),
        Err(e) => FffResult::err(&format!("Failed to serialize progress: {}", e)),
    }
}

/// Wait for initial scan to complete
#[unsafe(no_mangle)]
pub extern "C" fn fff_wait_for_scan(timeout_ms: u64) -> *mut FffResult {
    let file_picker = match FILE_PICKER.read() {
        Ok(f) => f,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    let picker = match file_picker.as_ref() {
        Some(p) => p,
        None => return FffResult::err("File picker not initialized"),
    };

    let timeout = Duration::from_millis(timeout_ms);
    let start = std::time::Instant::now();
    let mut sleep_duration = Duration::from_millis(1);

    while picker.is_scan_active() {
        if start.elapsed() >= timeout {
            return FffResult::ok_data("false");
        }
        std::thread::sleep(sleep_duration);
        sleep_duration = std::cmp::min(sleep_duration * 2, Duration::from_millis(50));
    }

    FffResult::ok_data("true")
}

/// Restart indexing in a new directory
///
/// # Safety
/// `new_path` must be a valid null-terminated UTF-8 string
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_restart_index(new_path: *const c_char) -> *mut FffResult {
    let path_str = match unsafe { cstr_to_str(new_path) } {
        Some(s) => s,
        None => return FffResult::err("Path is null or invalid UTF-8"),
    };

    let path = PathBuf::from(&path_str);
    if !path.exists() {
        return FffResult::err(&format!("Path does not exist: {}", path_str));
    }

    let canonical_path = match fff_core::path_utils::canonicalize(&path) {
        Ok(p) => p,
        Err(e) => return FffResult::err(&format!("Failed to canonicalize path: {}", e)),
    };

    let mut file_picker = match FILE_PICKER.write() {
        Ok(f) => f,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    // Stop existing picker, preserving warmup setting
    let warmup = if let Some(mut picker) = file_picker.take() {
        let warmup = picker.warmup_mmap_cache();
        picker.stop_background_monitor();
        warmup
    } else {
        false
    };

    // Create new picker
    match FilePicker::with_options(canonical_path.to_string_lossy().to_string(), warmup) {
        Ok(picker) => {
            *file_picker = Some(picker);
            FffResult::ok_empty()
        }
        Err(e) => FffResult::err(&format!("Failed to init file picker: {}", e)),
    }
}

// ============================================================================
// Frecency Functions
// ============================================================================

/// Track file access for frecency scoring
///
/// # Safety
/// `file_path` must be a valid null-terminated UTF-8 string
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_track_access(file_path: *const c_char) -> *mut FffResult {
    let path_str = match unsafe { cstr_to_str(file_path) } {
        Some(s) => s,
        None => return FffResult::err("File path is null or invalid UTF-8"),
    };

    let file_path = PathBuf::from(&path_str);

    // Track in frecency DB
    let frecency_guard = match FRECENCY.read() {
        Ok(f) => f,
        Err(e) => return FffResult::err(&format!("Failed to acquire frecency lock: {}", e)),
    };

    let frecency = match frecency_guard.as_ref() {
        Some(f) => f,
        None => return FffResult::ok_data("false"), // Frecency not initialized, skip
    };

    if let Err(e) = frecency.track_access(&file_path) {
        return FffResult::err(&format!("Failed to track access: {}", e));
    }
    drop(frecency_guard);

    // Update in file picker
    let mut file_picker = match FILE_PICKER.write() {
        Ok(f) => f,
        Err(e) => return FffResult::err(&format!("Failed to acquire file picker lock: {}", e)),
    };

    let picker = match file_picker.as_mut() {
        Some(p) => p,
        None => return FffResult::ok_data("false"),
    };

    let frecency_guard = match FRECENCY.read() {
        Ok(f) => f,
        Err(_) => return FffResult::ok_data("false"),
    };

    if let Some(ref frecency) = *frecency_guard {
        let _ = picker.update_single_file_frecency(&file_path, frecency);
    }

    FffResult::ok_data("true")
}

// ============================================================================
// Git Functions
// ============================================================================

/// Refresh git status cache
#[unsafe(no_mangle)]
pub extern "C" fn fff_refresh_git_status() -> *mut FffResult {
    match FilePicker::refresh_git_status_global() {
        Ok(count) => FffResult::ok_data(&count.to_string()),
        Err(e) => FffResult::err(&format!("Failed to refresh git status: {}", e)),
    }
}

// ============================================================================
// Query Tracking Functions
// ============================================================================

/// Track query completion for smart suggestions
///
/// # Safety
/// `query` and `file_path` must be valid null-terminated UTF-8 strings
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_track_query(
    query: *const c_char,
    file_path: *const c_char,
) -> *mut FffResult {
    let query_str = match unsafe { cstr_to_str(query) } {
        Some(s) => s,
        None => return FffResult::err("Query is null or invalid UTF-8"),
    };

    let path_str = match unsafe { cstr_to_str(file_path) } {
        Some(s) => s,
        None => return FffResult::err("File path is null or invalid UTF-8"),
    };

    let file_path = match fff_core::path_utils::canonicalize(path_str) {
        Ok(p) => p,
        Err(e) => return FffResult::err(&format!("Failed to canonicalize path: {}", e)),
    };

    let project_path = {
        let file_picker = match FILE_PICKER.read() {
            Ok(f) => f,
            Err(_) => return FffResult::ok_data("false"),
        };
        match file_picker.as_ref() {
            Some(p) => p.base_path().to_path_buf(),
            None => return FffResult::ok_data("false"),
        }
    };

    let mut query_tracker = match QUERY_TRACKER.write() {
        Ok(q) => q,
        Err(_) => return FffResult::ok_data("false"),
    };

    if let Some(ref mut tracker) = *query_tracker
        && let Err(e) = tracker.track_query_completion(query_str, &project_path, &file_path)
    {
        return FffResult::err(&format!("Failed to track query: {}", e));
    }

    FffResult::ok_data("true")
}

/// Get historical query by offset (0 = most recent)
#[unsafe(no_mangle)]
pub extern "C" fn fff_get_historical_query(offset: u64) -> *mut FffResult {
    let project_path = {
        let file_picker = match FILE_PICKER.read() {
            Ok(f) => f,
            Err(_) => return FffResult::ok_data("null"),
        };
        match file_picker.as_ref() {
            Some(p) => p.base_path().to_path_buf(),
            None => return FffResult::ok_data("null"),
        }
    };

    let query_tracker = match QUERY_TRACKER.read() {
        Ok(q) => q,
        Err(_) => return FffResult::ok_data("null"),
    };

    let tracker = match query_tracker.as_ref() {
        Some(t) => t,
        None => return FffResult::ok_data("null"),
    };

    match tracker.get_historical_query(&project_path, offset as usize) {
        Ok(Some(query)) => {
            let json = serde_json::to_string(&query).unwrap_or_else(|_| "null".to_string());
            FffResult::ok_data(&json)
        }
        Ok(None) => FffResult::ok_data("null"),
        Err(e) => FffResult::err(&format!("Failed to get historical query: {}", e)),
    }
}

/// Get health check information
///
/// # Safety
/// `test_path` can be null or a valid null-terminated UTF-8 string
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_health_check(test_path: *const c_char) -> *mut FffResult {
    let test_path = unsafe { cstr_to_str(test_path) }
        .filter(|s| !s.is_empty())
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

    // File picker info
    let mut picker_info = serde_json::Map::new();
    match FILE_PICKER.read() {
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
    health.insert(
        "file_picker".to_string(),
        serde_json::Value::Object(picker_info),
    );

    // Frecency info
    let mut frecency_info = serde_json::Map::new();
    match FRECENCY.read() {
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
    health.insert(
        "frecency".to_string(),
        serde_json::Value::Object(frecency_info),
    );

    // Query tracker info
    let mut query_info = serde_json::Map::new();
    match QUERY_TRACKER.read() {
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
    health.insert(
        "query_tracker".to_string(),
        serde_json::Value::Object(query_info),
    );

    match serde_json::to_string(&health) {
        Ok(json) => FffResult::ok_data(&json),
        Err(e) => FffResult::err(&format!("Failed to serialize health check: {}", e)),
    }
}

/// Free a result returned by any fff_* function
///
/// # Safety
/// `result_ptr` must be a valid pointer returned by a fff_* function
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_free_result(result_ptr: *mut FffResult) {
    if result_ptr.is_null() {
        return;
    }

    unsafe {
        let result = Box::from_raw(result_ptr);
        if !result.data.is_null() {
            drop(CString::from_raw(result.data));
        }
        if !result.error.is_null() {
            drop(CString::from_raw(result.error));
        }
        // Box will be dropped here, freeing the FffResult struct
    }
}

/// Free a string returned by fff_* functions
///
/// # Safety
/// `s` must be a valid C string allocated by this library
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_free_string(s: *mut c_char) {
    unsafe {
        if !s.is_null() {
            drop(CString::from_raw(s));
        }
    }
}
