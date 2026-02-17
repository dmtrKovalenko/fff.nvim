//! fff-core - High-performance file finder library
//!
//! This crate provides the core file indexing and fuzzy search functionality.
//!
//! # State management
//!
//! The library supports two usage modes:
//!
//! 1. **Global state** (for fff-nvim): Uses process-wide statics (`FILE_PICKER`, `FRECENCY`,
//!    `QUERY_TRACKER`). Only one instance can exist at a time.
//!
//! 2. **Instance state** (for fff-c / FFI consumers): Callers create their own
//!    `SharedPicker` / `SharedFrecency` and pass them into `FilePicker::with_shared_state`.
//!    Multiple independent instances can coexist.

mod background_watcher;
pub mod constraints;
mod db_healthcheck;
mod error;
pub mod file_picker;
pub mod frecency;
pub mod git;
pub mod grep;
pub mod path_utils;
pub mod query_tracker;
pub mod score;
mod sort_buffer;
pub mod types;

use file_picker::FilePicker;
use frecency::FrecencyTracker;
use once_cell::sync::Lazy;
use query_tracker::QueryTracker;
use std::sync::{Arc, RwLock};

/// Thread-safe shared handle to a `FilePicker`.
///
/// Used by `spawn_scan_and_watcher` and `BackgroundWatcher` to write scan results
/// and file-system events back into the picker without relying on global statics.
pub type SharedPicker = Arc<RwLock<Option<FilePicker>>>;

/// Thread-safe shared handle to a `FrecencyTracker`.
pub type SharedFrecency = Arc<RwLock<Option<FrecencyTracker>>>;

// Global state - same pattern as fff-nvim
pub static FRECENCY: Lazy<RwLock<Option<FrecencyTracker>>> = Lazy::new(|| RwLock::new(None));
pub static FILE_PICKER: Lazy<RwLock<Option<FilePicker>>> = Lazy::new(|| RwLock::new(None));
pub static QUERY_TRACKER: Lazy<RwLock<Option<QueryTracker>>> = Lazy::new(|| RwLock::new(None));

// Re-export main types for convenience
pub use db_healthcheck::{DbHealth, DbHealthChecker};
pub use error::{Error, Result};
pub use file_picker::{FuzzySearchOptions, ScanProgress};
pub use types::{FileItem, PaginationArgs, Score, ScoringContext, SearchResult};

// Re-export grep types
pub use grep::{GrepMatch, GrepMode, GrepResult, GrepSearchOptions};

// Re-export query parser types (including Location which moved there)
pub use fff_query_parser::{
    Constraint, FFFQuery, FuzzyQuery, Location, QueryParser, location::parse_location,
};
