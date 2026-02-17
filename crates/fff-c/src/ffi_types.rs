//! FFI-compatible type definitions
//!
//! These types use #[repr(C)] for C ABI compatibility and implement
//! serde traits for JSON serialization.

use std::ffi::{CString, c_char, c_void};
use std::ptr;

use fff_core::git::format_git_status;
use fff_core::{FileItem, GrepMatch, GrepResult, Location, Score, SearchResult};
use serde::{Deserialize, Serialize};

/// Result type returned by all FFI functions
/// Returned as a heap-allocated pointer that must be freed with fff_free_result
#[repr(C)]
pub struct FffResult {
    /// Whether the operation succeeded
    pub success: bool,
    /// JSON data on success (null-terminated string, caller must free)
    pub data: *mut c_char,
    /// Error message on failure (null-terminated string, caller must free)
    pub error: *mut c_char,
    /// Opaque handle pointer (used by fff_create to return the instance)
    pub handle: *mut c_void,
}

impl FffResult {
    /// Create a successful result with no data, returned as heap pointer
    pub fn ok_empty() -> *mut Self {
        Box::into_raw(Box::new(FffResult {
            success: true,
            data: ptr::null_mut(),
            error: ptr::null_mut(),
            handle: ptr::null_mut(),
        }))
    }

    /// Create a successful result with data, returned as heap pointer
    pub fn ok_data(data: &str) -> *mut Self {
        Box::into_raw(Box::new(FffResult {
            success: true,
            data: CString::new(data).unwrap_or_default().into_raw(),
            error: ptr::null_mut(),
            handle: ptr::null_mut(),
        }))
    }

    /// Create a successful result carrying an opaque instance handle.
    pub fn ok_handle(handle: *mut c_void) -> *mut Self {
        Box::into_raw(Box::new(FffResult {
            success: true,
            data: ptr::null_mut(),
            error: ptr::null_mut(),
            handle,
        }))
    }

    /// Create an error result, returned as heap pointer
    pub fn err(error: &str) -> *mut Self {
        Box::into_raw(Box::new(FffResult {
            success: false,
            data: ptr::null_mut(),
            error: CString::new(error).unwrap_or_default().into_raw(),
            handle: ptr::null_mut(),
        }))
    }
}

/// Initialization options (JSON-deserializable)
#[derive(Debug, Deserialize)]
pub struct InitOptions {
    /// Base directory to index (required)
    pub base_path: String,
    /// Path to frecency database (optional, omit to skip frecency initialization)
    pub frecency_db_path: Option<String>,
    /// Path to query history database (optional, omit to skip query tracker initialization)
    pub history_db_path: Option<String>,
    /// Use unsafe no-lock mode for databases (optional, defaults to false)
    #[serde(default)]
    pub use_unsafe_no_lock: bool,
    /// Pre-populate mmap caches for all files after initial scan so the first
    /// grep search is as fast as subsequent ones (optional, defaults to false)
    #[serde(default)]
    pub warmup_mmap_cache: bool,
}

/// Search options (JSON-deserializable)
#[derive(Debug, Default, Deserialize)]
pub struct SearchOptions {
    /// Maximum threads for parallel search (0 = auto)
    pub max_threads: Option<usize>,
    /// Current file path (for deprioritization)
    pub current_file: Option<String>,
    /// Combo boost score multiplier
    pub combo_boost_multiplier: Option<i32>,
    /// Minimum combo count for boost
    pub min_combo_count: Option<u32>,
    /// Page index for pagination
    pub page_index: Option<usize>,
    /// Page size for pagination
    pub page_size: Option<usize>,
}

/// Scan progress (JSON-serializable)
#[derive(Debug, Serialize)]
pub struct ScanProgress {
    pub scanned_files_count: usize,
    pub is_scanning: bool,
}

/// File item for JSON serialization
#[derive(Debug, Serialize)]
pub struct FileItemJson {
    pub path: String,
    pub relative_path: String,
    pub file_name: String,
    pub size: u64,
    pub modified: u64,
    pub access_frecency_score: i64,
    pub modification_frecency_score: i64,
    pub total_frecency_score: i64,
    pub git_status: String,
    pub is_binary: bool,
}

impl FileItemJson {
    pub fn from_file_item(item: &FileItem) -> Self {
        FileItemJson {
            path: item.path.to_string_lossy().to_string(),
            relative_path: item.relative_path.clone(),
            file_name: item.file_name.clone(),
            size: item.size,
            modified: item.modified,
            access_frecency_score: item.access_frecency_score,
            modification_frecency_score: item.modification_frecency_score,
            total_frecency_score: item.total_frecency_score,
            git_status: format_git_status(item.git_status).to_string(),
            is_binary: item.is_binary,
        }
    }
}

/// Score for JSON serialization
#[derive(Debug, Serialize)]
pub struct ScoreJson {
    pub total: i32,
    pub base_score: i32,
    pub filename_bonus: i32,
    pub special_filename_bonus: i32,
    pub frecency_boost: i32,
    pub distance_penalty: i32,
    pub current_file_penalty: i32,
    pub combo_match_boost: i32,
    pub exact_match: bool,
    pub match_type: String,
}

impl ScoreJson {
    pub fn from_score(score: &Score) -> Self {
        ScoreJson {
            total: score.total,
            base_score: score.base_score,
            filename_bonus: score.filename_bonus,
            special_filename_bonus: score.special_filename_bonus,
            frecency_boost: score.frecency_boost,
            distance_penalty: score.distance_penalty,
            current_file_penalty: score.current_file_penalty,
            combo_match_boost: score.combo_match_boost,
            exact_match: score.exact_match,
            match_type: score.match_type.to_string(),
        }
    }
}

/// Location for JSON serialization
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum LocationJson {
    #[serde(rename = "line")]
    Line { line: i32 },
    #[serde(rename = "position")]
    Position { line: i32, col: i32 },
    #[serde(rename = "range")]
    Range {
        start: PositionJson,
        end: PositionJson,
    },
}

#[derive(Debug, Serialize)]
pub struct PositionJson {
    pub line: i32,
    pub col: i32,
}

impl LocationJson {
    pub fn from_location(loc: &Location) -> Self {
        match loc {
            Location::Line(line) => LocationJson::Line { line: *line },
            Location::Position { line, col } => LocationJson::Position {
                line: *line,
                col: *col,
            },
            Location::Range { start, end } => LocationJson::Range {
                start: PositionJson {
                    line: start.0,
                    col: start.1,
                },
                end: PositionJson {
                    line: end.0,
                    col: end.1,
                },
            },
        }
    }
}

/// Search result for JSON serialization
#[derive(Debug, Serialize)]
pub struct SearchResultJson {
    pub items: Vec<FileItemJson>,
    pub scores: Vec<ScoreJson>,
    pub total_matched: usize,
    pub total_files: usize,
    pub location: Option<LocationJson>,
}

impl SearchResultJson {
    pub fn from_search_result(result: &SearchResult) -> Self {
        SearchResultJson {
            items: result
                .items
                .iter()
                .map(|item| FileItemJson::from_file_item(item))
                .collect(),
            scores: result.scores.iter().map(ScoreJson::from_score).collect(),
            total_matched: result.total_matched,
            total_files: result.total_files,
            location: result.location.as_ref().map(LocationJson::from_location),
        }
    }
}

// ============================================================================
// Grep (live search) types
// ============================================================================

/// Grep search options (JSON-deserializable)
#[derive(Debug, Default, Deserialize)]
pub struct GrepSearchOptionsJson {
    /// Maximum file size to search (bytes, default: 10MB)
    pub max_file_size: Option<u64>,
    /// Maximum matches per file (default: 200)
    pub max_matches_per_file: Option<usize>,
    /// Smart case: case-insensitive if query is lowercase (default: true)
    pub smart_case: Option<bool>,
    /// File-based pagination offset (default: 0)
    pub file_offset: Option<usize>,
    /// Maximum matches to return (default: 50)
    pub page_limit: Option<usize>,
    /// Search mode: "plain", "regex", or "fuzzy" (default: "plain")
    pub mode: Option<String>,
    /// Time budget in milliseconds, 0 = unlimited (default: 0)
    pub time_budget_ms: Option<u64>,
}

/// A single grep match for JSON serialization
#[derive(Debug, Serialize)]
pub struct GrepMatchJson {
    /// File metadata
    pub path: String,
    pub relative_path: String,
    pub file_name: String,
    pub git_status: String,
    pub size: u64,
    pub modified: u64,
    pub is_binary: bool,
    pub total_frecency_score: i64,
    pub access_frecency_score: i64,
    pub modification_frecency_score: i64,
    /// Match metadata
    pub line_number: u64,
    pub col: usize,
    pub byte_offset: u64,
    pub line_content: String,
    /// Byte offset pairs (start, end) within line_content for highlighting
    pub match_ranges: Vec<[u32; 2]>,
    /// Fuzzy match score (only in fuzzy mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuzzy_score: Option<u16>,
}

impl GrepMatchJson {
    pub fn from_grep_match(m: &GrepMatch, file: &FileItem) -> Self {
        GrepMatchJson {
            path: file.path.to_string_lossy().to_string(),
            relative_path: file.relative_path.clone(),
            file_name: file.file_name.clone(),
            git_status: format_git_status(file.git_status).to_string(),
            size: file.size,
            modified: file.modified,
            is_binary: file.is_binary,
            total_frecency_score: file.total_frecency_score,
            access_frecency_score: file.access_frecency_score,
            modification_frecency_score: file.modification_frecency_score,
            line_number: m.line_number,
            col: m.col,
            byte_offset: m.byte_offset,
            line_content: m.line_content.clone(),
            match_ranges: m
                .match_byte_offsets
                .iter()
                .map(|&(start, end)| [start, end])
                .collect(),
            fuzzy_score: m.fuzzy_score,
        }
    }
}

/// Grep result for JSON serialization
#[derive(Debug, Serialize)]
pub struct GrepResultJson {
    pub items: Vec<GrepMatchJson>,
    pub total_matched: usize,
    pub total_files_searched: usize,
    pub total_files: usize,
    pub filtered_file_count: usize,
    pub next_file_offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regex_fallback_error: Option<String>,
}

impl GrepResultJson {
    pub fn from_grep_result(result: &GrepResult) -> Self {
        GrepResultJson {
            items: result
                .matches
                .iter()
                .map(|m| {
                    let file = result.files[m.file_index];
                    GrepMatchJson::from_grep_match(m, file)
                })
                .collect(),
            total_matched: result.matches.len(),
            total_files_searched: result.total_files_searched,
            total_files: result.total_files,
            filtered_file_count: result.filtered_file_count,
            next_file_offset: result.next_file_offset,
            regex_fallback_error: result.regex_fallback_error.clone(),
        }
    }
}
