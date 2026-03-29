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
//! [`SharedPicker`], [`SharedFrecency`], and [`SharedQueryTracker`] are
//! `Arc<RwLock<Option<T>>>` type aliases for thread-safe shared access. FFF
//! is designed for long-running processes that keep the file index in global
//! state, so these wrappers let background threads (scanner, watcher) share
//! data with the calling code safely.
//!
//! ## Quick Start
//!
//! ```
//! use fff_search::file_picker::FilePicker;
//! use fff_search::frecency::FrecencyTracker;
//! use fff_search::query_tracker::QueryTracker;
//! use fff_search::{
//!     FFFMode, FuzzySearchOptions, PaginationArgs, QueryParser,
//!     SharedFrecency, SharedPicker, SharedQueryTracker,
//! };
//!
//! let shared_picker: SharedPicker = Default::default();
//! let shared_frecency: SharedFrecency = Default::default();
//! let shared_query_tracker: SharedQueryTracker = Default::default();
//!
//! let tmp = std::env::temp_dir().join("fff-doctest");
//! std::fs::create_dir_all(&tmp).unwrap();
//!
//! // 1. Optionally initialize frecency and query tracker databases
//! let frecency = FrecencyTracker::new(tmp.join("frecency"), false)?;
//! *shared_frecency.write().unwrap() = Some(frecency);
//!
//! let query_tracker = QueryTracker::new(tmp.join("queries"), false)?;
//! *shared_query_tracker.write().unwrap() = Some(query_tracker);
//!
//! // 2. Init the file picker (spawns background scan + watcher)
//! FilePicker::new_with_shared_state(
//!     ".".into(),
//!     /* warmup memap caches = */ false,
//!     FFFMode::Ai, // use AI for ai agents, and Neovim for editors
//!     shared_picker.clone(),
//!     shared_frecency.clone(),
//! )?;
//!
//! // 3. Wait for scan (in real app you would like to add some tokio flavor here)
//! let _ = FilePicker::wait_for_scan(&shared_picker, std::time::Duration::from_secs(10));
//!
//! // 4. Search: lock the picker and query tracker
//! let picker_lock_guard = shared_picker.read().unwrap();
//! let picker = picker_lock_guard.as_ref().unwrap();
//! let query_tracker_lock_guard = shared_query_tracker.read().unwrap();
//!
//! // 5. Parse the query and perform fuzzy search with frecency and combo-boost scoring
//! let parser = QueryParser::default();
//! let query = parser.parse("lib.rs");
//!
//! let results = FilePicker::fuzzy_search(
//!     picker.get_files(),
//!     &query,
//!     query_tracker_lock_guard.as_ref(),
//!     FuzzySearchOptions {
//!         max_threads: 0,
//!         current_file: None,
//!         pagination: PaginationArgs { offset: 0, limit: 50 },
//!         ..Default::default()
//!     },
//! );
//!
//! assert!(results.total_matched > 0);
//! assert!(results.items.first().unwrap().path.ends_with("lib.rs"));
//!
//! let _ = std::fs::remove_dir_all(&tmp);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod background_watcher;
mod constraints;
mod db_healthcheck;
mod error;
mod score;
mod sort_buffer;

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

use std::sync::{Arc, RwLock};

/// Thread-safe shared handle to the [`FilePicker`] instance.
pub type SharedPicker = Arc<RwLock<Option<FilePicker>>>;

/// Thread-safe shared handle to the [`FrecencyTracker`] instance.
pub type SharedFrecency = Arc<RwLock<Option<FrecencyTracker>>>;

/// Thread-safe shared handle to the [`QueryTracker`] instance.
pub type SharedQueryTracker = Arc<RwLock<Option<QueryTracker>>>;

pub use db_healthcheck::{DbHealth, DbHealthChecker};
pub use error::{Error, Result};
pub use fff_query_parser::*;
pub use file_picker::*;
pub use frecency::*;
pub use grep::*;
pub use query_tracker::*;
pub use types::*;
