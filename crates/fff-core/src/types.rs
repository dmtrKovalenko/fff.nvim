use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use memmap2::Mmap;

use crate::constraints::Constrainable;
use crate::query_tracker::QueryMatchEntry;
use fff_query_parser::{FFFQuery, FuzzyQuery, Location};

/// A single indexed file with metadata, frecency scores, and lazy mmap.
///
/// The `mmap` field holds the memory-mapped file contents, initialized lazily
/// on the first grep access and cached for subsequent searches. The mmap is
/// backed by the kernel page cache and automatically reflects file modifications
/// — no manual invalidation is needed.
///
/// Thread-safety: `OnceLock` provides lock-free reads after initialization.
/// Each file is only searched by one rayon worker at a time via `par_iter`.
#[derive(Debug)]
pub struct FileItem {
    pub path: PathBuf,
    pub relative_path: String,
    pub relative_path_lower: String,
    pub file_name: String,
    pub file_name_lower: String,
    pub size: u64,
    pub modified: u64,
    pub access_frecency_score: i64,
    pub modification_frecency_score: i64,
    pub total_frecency_score: i64,
    pub git_status: Option<git2::Status>,
    pub is_binary: bool,
    /// Lazily-initialized memory-mapped file contents for grep.
    /// Initialized on first grep access via `OnceLock`; lock-free on subsequent reads.
    /// Automatically reflects file changes via the kernel page cache.
    mmap: OnceLock<Mmap>,
}

impl Clone for FileItem {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            relative_path: self.relative_path.clone(),
            relative_path_lower: self.relative_path_lower.clone(),
            file_name: self.file_name.clone(),
            file_name_lower: self.file_name_lower.clone(),
            size: self.size,
            modified: self.modified,
            access_frecency_score: self.access_frecency_score,
            modification_frecency_score: self.modification_frecency_score,
            total_frecency_score: self.total_frecency_score,
            git_status: self.git_status,
            is_binary: self.is_binary,
            // Don't clone the mmap — the clone lazily re-creates it on demand
            mmap: OnceLock::new(),
        }
    }
}

const MAX_MMAP_FILE_SIZE: u64 = 10 * 1024 * 1024;

impl FileItem {
    /// Create a new `FileItem` with all fields specified and an empty (not yet loaded) mmap.
    pub fn new_raw(
        path: PathBuf,
        relative_path: String,
        file_name: String,
        size: u64,
        modified: u64,
        git_status: Option<git2::Status>,
        is_binary: bool,
    ) -> Self {
        Self {
            relative_path_lower: relative_path.to_lowercase(),
            file_name_lower: file_name.to_lowercase(),
            path,
            relative_path,
            file_name,
            size,
            modified,
            access_frecency_score: 0,
            modification_frecency_score: 0,
            total_frecency_score: 0,
            git_status,
            is_binary,
            mmap: OnceLock::new(),
        }
    }

    /// Invalidate the cached mmap so the next `get_mmap()` call creates a fresh one.
    ///
    /// Call this when the background watcher detects that the file has been modified.
    /// While the kernel page cache reflects content changes automatically, a file
    /// that is truncated (made smaller) while mapped can cause SIGBUS if the search
    /// accesses pages beyond the new file size. Invalidating the mmap ensures a
    /// fresh mapping with the correct size is created on the next access.
    pub fn invalidate_mmap(&mut self) {
        self.mmap = OnceLock::new();
    }

    /// Get the cached mmap or lazily create it. Returns `None` if the file
    /// is too large, empty, or can't be opened/mapped.
    ///
    /// After the first call, this is lock-free (just an atomic load + pointer deref).
    /// The mmap is backed by the kernel page cache and automatically reflects
    /// file modifications — no manual invalidation is needed.
    #[inline]
    pub fn get_mmap(&self) -> Option<&Mmap> {
        if let Some(mmap) = self.mmap.get() {
            return Some(mmap);
        }

        if self.size == 0 || self.size > MAX_MMAP_FILE_SIZE {
            return None;
        }

        let file = std::fs::File::open(&self.path).ok()?;
        // SAFETY: The mmap is backed by the kernel page cache and automatically
        // reflects file modifications. The only risk is SIGBUS if the file is
        // truncated while mapped
        let mmap = unsafe { Mmap::map(&file) }.ok()?;

        // If another thread raced us, OnceLock discards our mmap and returns theirs.
        // This is fine — the duplicate mmap is just dropped.
        Some(self.mmap.get_or_init(|| mmap))
    }
}

impl Constrainable for FileItem {
    #[inline]
    fn relative_path(&self) -> &str {
        &self.relative_path
    }

    #[inline]
    fn relative_path_lower(&self) -> &str {
        &self.relative_path_lower
    }

    #[inline]
    fn file_name(&self) -> &str {
        &self.file_name
    }

    #[inline]
    fn git_status(&self) -> Option<git2::Status> {
        self.git_status
    }
}

#[derive(Debug, Clone)]
pub struct Score {
    pub total: i32,
    pub base_score: i32,
    pub filename_bonus: i32,
    pub special_filename_bonus: i32,
    pub frecency_boost: i32,
    pub distance_penalty: i32,
    pub current_file_penalty: i32,
    pub combo_match_boost: i32,
    pub exact_match: bool,
    pub match_type: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct PaginationArgs {
    pub offset: usize,
    pub limit: usize,
}

/// Context for scoring files during search.
///
/// The `parsed_query` field contains the pre-parsed query with constraints,
/// fuzzy parts, and location information. Parsing is done once at the API
/// boundary and passed through.
#[derive(Debug, Clone)]
pub struct ScoringContext<'a> {
    /// The original raw query string (for compatibility and debugging)
    pub raw_query: &'a str,
    /// Pre-parsed query containing constraints, fuzzy parts, and location
    pub parsed_query: Option<FFFQuery<'a>>,
    pub project_path: Option<&'a Path>,
    pub current_file: Option<&'a str>,
    pub max_typos: u16,
    pub max_threads: usize,
    pub last_same_query_match: Option<&'a QueryMatchEntry>,
    pub combo_boost_score_multiplier: i32,
    pub min_combo_count: u32,
    pub pagination: PaginationArgs,
}

impl<'a> ScoringContext<'a> {
    /// Get the effective fuzzy query string for matching.
    /// Returns the first fuzzy part, or the raw query if no parsing was done.
    pub fn effective_query(&self) -> &'a str {
        match &self.parsed_query {
            Some(p) => match &p.fuzzy_query {
                FuzzyQuery::Text(t) => t,
                FuzzyQuery::Parts(parts) if !parts.is_empty() => parts[0],
                _ => self.raw_query.trim(),
            },
            None => self.raw_query.trim(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchResult<'a> {
    pub items: Vec<&'a FileItem>,
    pub scores: Vec<Score>,
    pub total_matched: usize,
    pub total_files: usize,
    pub location: Option<Location>,
}
