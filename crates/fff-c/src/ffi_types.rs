//! FFI-compatible type definitions
//!
//! These types use #[repr(C)] for C ABI compatibility and implement
//! serde traits for JSON serialization.

use std::ffi::{CString, c_char};
use std::ptr;

use fff_core::git::format_git_status;
use fff_core::{FileItem, Location, Score, SearchResult};
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
}

impl FffResult {
    /// Create a successful result with no data, returned as heap pointer
    pub fn ok_empty() -> *mut Self {
        Box::into_raw(Box::new(FffResult {
            success: true,
            data: ptr::null_mut(),
            error: ptr::null_mut(),
        }))
    }

    /// Create a successful result with data, returned as heap pointer
    pub fn ok_data(data: &str) -> *mut Self {
        Box::into_raw(Box::new(FffResult {
            success: true,
            data: CString::new(data).unwrap_or_default().into_raw(),
            error: ptr::null_mut(),
        }))
    }

    /// Create an error result, returned as heap pointer
    pub fn err(error: &str) -> *mut Self {
        Box::into_raw(Box::new(FffResult {
            success: false,
            data: ptr::null_mut(),
            error: CString::new(error).unwrap_or_default().into_raw(),
        }))
    }
}

/// Initialization options (JSON-deserializable)
#[derive(Debug, Deserialize)]
pub struct InitOptions {
    /// Base directory to index (required)
    pub base_path: String,
    /// Path to frecency database (optional, defaults to ~/.fff/frecency.mdb)
    pub frecency_db_path: Option<String>,
    /// Path to query history database (optional, defaults to ~/.fff/history.mdb)
    pub history_db_path: Option<String>,
    /// Use unsafe no-lock mode for databases (optional, defaults to false)
    #[serde(default)]
    pub use_unsafe_no_lock: bool,
    /// Skip database initialization entirely (optional, defaults to false)
    #[serde(default)]
    pub skip_databases: bool,
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
            scores: result
                .scores
                .iter()
                .map(ScoreJson::from_score)
                .collect(),
            total_matched: result.total_matched,
            total_files: result.total_files,
            location: result.location.as_ref().map(LocationJson::from_location),
        }
    }
}
