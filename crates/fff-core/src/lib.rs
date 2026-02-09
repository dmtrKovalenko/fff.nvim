//! fff-core - High-performance file finder library
//!
//! This crate provides the core file indexing and fuzzy search functionality.
//! It maintains global state for the file picker, frecency tracker, and query tracker.

mod background_watcher;
mod db_healthcheck;
mod error;
pub mod file_picker;
pub mod frecency;
pub mod git;
pub mod path_utils;
pub mod query_tracker;
pub mod score;
mod sort_buffer;
pub mod types;

use file_picker::FilePicker;
use frecency::FrecencyTracker;
use once_cell::sync::Lazy;
use query_tracker::QueryTracker;
use std::sync::RwLock;

// Global state - same pattern as fff-nvim
pub static FRECENCY: Lazy<RwLock<Option<FrecencyTracker>>> = Lazy::new(|| RwLock::new(None));
pub static FILE_PICKER: Lazy<RwLock<Option<FilePicker>>> = Lazy::new(|| RwLock::new(None));
pub static QUERY_TRACKER: Lazy<RwLock<Option<QueryTracker>>> = Lazy::new(|| RwLock::new(None));

// Re-export main types for convenience
pub use db_healthcheck::{DbHealth, DbHealthChecker};
pub use error::{Error, Result};
pub use file_picker::{FuzzySearchOptions, ScanProgress};
pub use path_utils::{PathShortenStrategy, shorten_path};
pub use types::{FileItem, PaginationArgs, Score, ScoringContext, SearchResult};

// Re-export query parser types (including Location which moved there)
pub use fff_query_parser::{
    Constraint, FuzzyQuery, Location, ParseResult, QueryParser, location::parse_location,
};
