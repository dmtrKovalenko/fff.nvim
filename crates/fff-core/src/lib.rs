//! # FFF Search — High-performance file finder core
//!
//! This crate provides the core search engine for [FFF (Fast File Finder)](https://github.com/dmtrKovalenko/fff.nvim).
//! It includes filesystem indexing with real-time watching, fuzzy matching powered
//! by [frizbee](https://docs.rs/neo_frizbee), frecency scoring backed by LMDB,
//! and multi-mode grep search.
//!
//! ## Architecture
//!
//! - [`file_picker::FilePicker`] — Main entry point. Indexes a directory tree in a
//!   background thread, maintains a sorted file list, watches the filesystem for
//!   changes, and performs fuzzy search with frecency-weighted scoring.
//! - [`frecency::FrecencyTracker`] — LMDB-backed database that tracks file access
//!   and modification patterns for intelligent result ranking.
//! - [`query_tracker::QueryTracker`] — Tracks search query history and provides
//!   "combo-boost" scoring for repeatedly matched files.
//! - [`grep`] — Live grep search supporting regex, plain-text, and fuzzy modes
//!   with optional constraint filtering.
//! - [`git`] — Git status caching and repository detection.
//!
//! ## Shared State
//!
//! [`SharedFilePicker`], [`SharedFrecency`], and [`SharedQueryTracker`] are
//! newtype wrappers around `Arc<RwLock<Option<T>>>` for thread-safe shared
//! access. They provide `read()` / `write()` methods with built-in error
//! conversion and convenience helpers like `wait_for_scan()`.
//!
//! ## Quick Start
//!
//! ```
//! use fff_search::file_picker::FilePicker;
//! use fff_search::frecency::FrecencyTracker;
//! use fff_search::query_tracker::QueryTracker;
//! use fff_search::{
//!     FFFMode, FilePickerOptions, FuzzySearchOptions, PaginationArgs, QueryParser,
//!     SharedFrecency, SharedFilePicker, SharedQueryTracker,
//! };
//!
//! let shared_picker = SharedFilePicker::default();
//! let shared_frecency = SharedFrecency::default();
//! let shared_query_tracker = SharedQueryTracker::default();
//!
//! let tmp = std::env::temp_dir().join("fff-doctest");
//! std::fs::create_dir_all(&tmp).unwrap();
//!
//! // 1. Optionally initialize frecency and query tracker databases
//! let frecency = FrecencyTracker::open(tmp.join("frecency"))?;
//! shared_frecency.init(frecency)?;
//!
//! let query_tracker = QueryTracker::open(tmp.join("queries"))?;
//! shared_query_tracker.init(query_tracker)?;
//!
//! // 2. Init the file picker (spawns background scan + watcher)
//! FilePicker::new_with_shared_state(
//!     shared_picker.clone(),
//!     shared_frecency.clone(),
//!     FilePickerOptions {
//!         base_path: ".".into(),
//!         mode: FFFMode::Ai,
//!         ..Default::default()
//!     },
//! )?;
//!
//! // 3. Wait for scan
//! shared_picker.wait_for_scan(std::time::Duration::from_secs(10));
//!
//! // 4. Search: lock the picker and query tracker
//! let picker_guard = shared_picker.read()?;
//! let picker = picker_guard.as_ref().unwrap();
//! let qt_guard = shared_query_tracker.read()?;
//!
//! // 5. Parse the query and perform fuzzy search
//! let parser = QueryParser::default();
//! let query = parser.parse("lib.rs");
//!
//! let results = picker.fuzzy_search(
//!     &query,
//!     qt_guard.as_ref(),
//!     FuzzySearchOptions {
//!         max_threads: 0,
//!         current_file: None,
//!         pagination: PaginationArgs { offset: 0, limit: 50 },
//!         ..Default::default()
//!     },
//! );
//!
//! assert!(results.total_matched > 0);
//! assert!(results.items.first().unwrap().relative_path(picker).ends_with("lib.rs"));
//!
//! let _ = std::fs::remove_dir_all(&tmp);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod background_watcher;
mod scan;
// public only for benchmarks — the inverted index is still re-exported via
// `pub use bigram_filter::*` below for external consumers.
#[doc(hidden)]
pub mod bigram_filter;
pub mod bigram_query;
mod constraints;
mod db_healthcheck;
mod error;
mod score;
mod sort_buffer;
// this is pub only for benchmarks
pub mod case_insensitive_memmem;

pub(crate) mod simd_path;

/// Core file picker: filesystem indexing, background watching, and fuzzy search.
///
/// See [`FilePicker`](file_picker::FilePicker) for the main entry point.
pub mod file_picker;

/// Frecency (frequency + recency) database for file access scoring.
///
/// Backed by LMDB for persistent, crash-safe storage.
pub mod frecency;

/// Git status caching and repository detection utilities.
pub mod git;

/// Live grep search with regex, plain-text, and fuzzy matching modes.
///
/// Supports constraint filtering (file extensions, path segments, globs)
/// and parallel execution via rayon.
pub mod grep;

/// Tracing/logging initialization and panic hook setup.
pub mod log;

/// Path manipulation utilities: cross platform canonicalization, tilde expansion, and
/// directory distance penalties for search scoring.
pub mod path_utils;

/// Search query history tracker for combo-boost scoring.
///
/// Records which files a user selects for each query, enabling the scorer
/// to boost files that were previously chosen for similar searches.
pub mod query_tracker;

/// Core data types shared across the crate.
pub mod types;

mod ignore;
mod lmdb;
/// Thread-safe shared handles for [`FilePicker`], [`FrecencyTracker`],
/// and [`QueryTracker`].
pub mod shared;

pub use bigram_filter::*;
pub use db_healthcheck::{DbHealth, DbHealthChecker};
pub use error::{Error, Result};
pub use fff_query_parser::*;
pub use file_picker::*;
pub use frecency::*;
pub use grep::*;
pub use query_tracker::*;
pub use shared::*;
pub use types::*;
