//! fff-core - High-performance file finder library
//!
//! This crate provides the core file indexing and fuzzy search functionality.
//! It maintains global state for the file picker, frecency tracker, and query tracker.

mod background_watcher;
pub mod constraints;
mod db_healthcheck;
mod error;
pub mod file_picker;
pub mod frecency;
pub mod git;
pub mod grep;
pub mod mmap_cache;
pub mod path_utils;
pub mod query_tracker;
pub mod score;
mod sort_buffer;
pub mod types;

use file_picker::FilePicker;
use frecency::FrecencyTracker;
use mmap_cache::MmapCache;
use once_cell::sync::Lazy;
use query_tracker::QueryTracker;
use std::sync::RwLock;

// Global state - same pattern as fff-nvim
pub static FRECENCY: Lazy<RwLock<Option<FrecencyTracker>>> = Lazy::new(|| RwLock::new(None));
pub static FILE_PICKER: Lazy<RwLock<Option<FilePicker>>> = Lazy::new(|| RwLock::new(None));
pub static QUERY_TRACKER: Lazy<RwLock<Option<QueryTracker>>> = Lazy::new(|| RwLock::new(None));

/// Global mmap cache for grep — no Option wrapper needed, starts empty and is always valid.
/// Default max file size: 10 MB.
pub static MMAP_CACHE: Lazy<MmapCache> = Lazy::new(|| MmapCache::new(10 * 1024 * 1024));

// Re-export main types for convenience
pub use db_healthcheck::{DbHealth, DbHealthChecker};
pub use error::{Error, Result};
pub use file_picker::{FuzzySearchOptions, ScanProgress};
pub use types::{FileItem, PaginationArgs, Score, ScoringContext, SearchResult};

// Re-export grep types
pub use grep::{GrepMatch, GrepResult, GrepSearchOptions};

// Re-export query parser types (including Location which moved there)
pub use fff_query_parser::{
    Constraint, FFFQuery, FuzzyQuery, Location, QueryParser, location::parse_location,
};
