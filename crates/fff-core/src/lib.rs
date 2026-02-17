//! fff-core - High-performance file finder library
//!
//! This crate provides the core file indexing and fuzzy search functionality.
//!
//! # State management
//!
//! All state is instance-based. Callers create their own `SharedPicker` /
//! `SharedFrecency` / `SharedQueryTracker` and pass them into
//! `FilePicker::new_with_shared_state`. Multiple independent instances can
//! coexist in the same process.

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
use query_tracker::QueryTracker;
use std::sync::{Arc, RwLock};

pub type SharedPicker = Arc<RwLock<Option<FilePicker>>>;
pub type SharedFrecency = Arc<RwLock<Option<FrecencyTracker>>>;
pub type SharedQueryTracker = Arc<RwLock<Option<QueryTracker>>>;

pub use db_healthcheck::{DbHealth, DbHealthChecker};
pub use error::{Error, Result};
pub use fff_query_parser::{
    Constraint, FFFQuery, FuzzyQuery, Location, QueryParser, location::parse_location,
};
pub use file_picker::{FuzzySearchOptions, ScanProgress};
pub use grep::{GrepMatch, GrepMode, GrepResult, GrepSearchOptions};
pub use types::{FileItem, PaginationArgs, Score, ScoringContext, SearchResult};
