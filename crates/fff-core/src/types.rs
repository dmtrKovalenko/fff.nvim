use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::constraints::Constrainable;
use crate::query_tracker::QueryMatchEntry;
use fff_query_parser::{FFFQuery, FuzzyQuery, Location};

/// Cached file contents — mmap on Unix, heap buffer on Windows.
///
/// On Windows, memory-mapped files hold the file handle open and prevent
/// editors from saving (writing/replacing) those files. Reading into a
/// `Vec<u8>` releases the handle immediately after the read completes.
#[derive(Debug)]
#[allow(dead_code)] // variants are conditionally used per platform
pub enum FileContent {
    #[cfg(not(target_os = "windows"))]
    Mmap(memmap2::Mmap),
    #[cfg(target_os = "windows")]
    Buffer(Vec<u8>),
}

impl std::ops::Deref for FileContent {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            #[cfg(not(target_os = "windows"))]
            FileContent::Mmap(m) => m,
            #[cfg(target_os = "windows")]
            FileContent::Buffer(b) => b,
        }
    }
}

/// A single indexed file with metadata, frecency scores, and lazy content cache.
///
/// File contents are initialized lazily on the first grep access and cached for
/// subsequent searches. On Unix, uses mmap backed by the kernel page cache. On
/// Windows, reads into a heap buffer to avoid holding file handles open.
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
    /// Lazily-initialized file contents for grep.
    /// Initialized on first grep access via `OnceLock`; lock-free on subsequent reads.
    content: OnceLock<FileContent>,
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
            // Don't clone the content — the clone lazily re-creates it on demand
            content: OnceLock::new(),
        }
    }
}

/// File content that is either borrowed from the persistent cache or owned
/// from a temporary mmap. Dereferences to `&[u8]` so callers can use it
/// transparently.
///
/// On Unix the uncached variant holds a temporary `memmap2::Mmap` that is
/// backed by the kernel page cache — same zero-copy benefit as the cached
/// path, but the mapping is released (munmap) as soon as this value is
/// dropped instead of being retained for the lifetime of the `FileItem`.
pub enum FileContentRef<'a> {
    /// Content is stored in the `FileItem`'s `OnceLock` cache (fast path).
    Cached(&'a [u8]),
    /// Temporary mmap (Unix) / heap buffer (Windows) created because the
    /// persistent cache budget was exceeded. Unmapped on drop.
    Temp(FileContent),
}

impl std::ops::Deref for FileContentRef<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            FileContentRef::Cached(s) => s,
            FileContentRef::Temp(c) => c,
        }
    }
}

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
            content: OnceLock::new(),
        }
    }

    /// Invalidate the cached content so the next `get_content()` call creates a fresh one.
    ///
    /// Call this when the background watcher detects that the file has been modified.
    /// On Unix, a file that is truncated while mapped can cause SIGBUS. On Windows,
    /// the stale buffer simply won't reflect the new contents. In both cases,
    /// invalidating ensures a fresh read on the next access.
    pub fn invalidate_mmap(&mut self, budget: &ContentCacheBudget) {
        if self.content.get().is_some() {
            budget.cached_count.fetch_sub(1, Ordering::Relaxed);
            budget.cached_bytes.fetch_sub(self.size, Ordering::Relaxed);
        }
        self.content = OnceLock::new();
    }

    /// Get the cached file contents or lazily load and cache them.
    ///
    /// Returns `None` if the file is too large, empty, can't be opened, **or
    /// the cache budget is exhausted**. Callers that need content regardless
    /// of the budget should use [`get_content_for_search`].
    ///
    /// After the first call, this is lock-free (just an atomic load + pointer deref).
    #[inline]
    pub fn get_content(&self, budget: &ContentCacheBudget) -> Option<&[u8]> {
        if let Some(content) = self.content.get() {
            return Some(content);
        }

        if self.size == 0 || self.size > MAX_MMAP_FILE_SIZE {
            return None;
        }

        // Check cache budget before creating a new persistent cache entry.
        let count = budget.cached_count.load(Ordering::Relaxed);
        let bytes = budget.cached_bytes.load(Ordering::Relaxed);
        if count >= budget.max_files || bytes + self.size > MAX_CACHED_CONTENT_BYTES {
            return None;
        }

        let content = load_file_content(&self.path)?;
        let result = self.content.get_or_init(|| content);

        // Bump counters. Slight over-count under races is fine — the budget
        // is a soft limit and the overshoot is bounded by rayon thread count.
        budget.cached_count.fetch_add(1, Ordering::Relaxed);
        budget.cached_bytes.fetch_add(self.size, Ordering::Relaxed);

        Some(result)
    }

    /// Backward-compatible alias for `get_content`.
    #[inline]
    pub fn get_mmap(&self, budget: &ContentCacheBudget) -> Option<&[u8]> {
        self.get_content(budget)
    }

    /// Get file content for searching — **always returns content** for eligible
    /// files, even when the persistent cache budget is exhausted.
    ///
    /// Tries the `OnceLock` cache first (fast path). If the cache is full,
    /// falls back to a temporary mmap that is unmapped when the returned
    /// [`FileContentRef`] is dropped — no persistent kernel resources retained.
    #[inline]
    pub fn get_content_for_search<'a>(
        &'a self,
        budget: &ContentCacheBudget,
    ) -> Option<FileContentRef<'a>> {
        if let Some(cached) = self.get_content(budget) {
            return Some(FileContentRef::Cached(cached));
        }

        // get_content returned None — either ineligible or over budget.
        if self.is_binary || self.size == 0 || self.size > MAX_MMAP_FILE_SIZE {
            return None;
        }

        // Over budget: create a temporary mmap that is unmapped on drop.
        let content = load_file_content(&self.path)?;
        Some(FileContentRef::Temp(content))
    }
}

/// Load file contents: mmap on Unix, heap buffer on Windows.
fn load_file_content(path: &Path) -> Option<FileContent> {
    #[cfg(not(target_os = "windows"))]
    {
        let file = std::fs::File::open(path).ok()?;
        // SAFETY: The mmap is backed by the kernel page cache and automatically
        // reflects file modifications. The only risk is SIGBUS if the file is
        // truncated while mapped.
        let mmap = unsafe { memmap2::Mmap::map(&file) }.ok()?;
        Some(FileContent::Mmap(mmap))
    }

    #[cfg(target_os = "windows")]
    {
        let data = std::fs::read(path).ok()?;
        Some(FileContent::Buffer(data))
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

#[derive(Debug, Clone, Default)]
pub struct Score {
    pub total: i32,
    pub base_score: i32,
    pub filename_bonus: i32,
    pub special_filename_bonus: i32,
    pub frecency_boost: i32,
    pub git_status_boost: i32,
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

impl Default for PaginationArgs {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 100,
        }
    }
}

/// Context for scoring files during search.
///
/// The `query` field contains the pre-parsed query with constraints,
/// fuzzy parts, and location information. Parsing is done once at the API
/// boundary and passed through.
#[derive(Debug, Clone)]
pub struct ScoringContext<'a> {
    /// Parsed query containing raw text, constraints, fuzzy parts, and location
    pub query: &'a FFFQuery<'a>,
    pub project_path: Option<&'a Path>,
    pub current_file: Option<&'a str>,
    pub max_typos: u16,
    pub max_threads: usize,
    pub last_same_query_match: Option<QueryMatchEntry>,
    pub combo_boost_score_multiplier: i32,
    pub min_combo_count: u32,
    pub pagination: PaginationArgs,
}

impl ScoringContext<'_> {
    /// Get the effective fuzzy query string for matching.
    /// Returns the first fuzzy part, or the raw query if no parsing was done.
    pub fn effective_query(&self) -> &str {
        match &self.query.fuzzy_query {
            FuzzyQuery::Text(t) => t,
            FuzzyQuery::Parts(parts) if !parts.is_empty() => parts[0],
            _ => self.query.raw_query.trim(),
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

const MAX_MMAP_FILE_SIZE: u64 = 10 * 1024 * 1024;

// Limits the total number of files (and bytes) whose content is kept in
// memory via the `OnceLock<FileContent>` cache. On Unix every cached file
// holds a live `mmap`, which consumes a kernel `vm_map_entry`. On a 500k-file
// monorepo, caching everything exhausts macOS/Linux kernel resources and
// crashes the machine (see issue #294).
//
// Each `FilePicker` owns its own `ContentCacheBudget`. The budget is passed
// to `grep_search` and `warmup_mmaps` so that multiple pickers can coexist
// without interfering with each other's counters.

const MAX_CACHED_CONTENT_BYTES: u64 = 512 * 1024 * 1024;

/// Per-picker budget controlling how many files may have their content
/// persistently cached (mmap on Unix, heap buffer on Windows).
#[derive(Debug)]
pub struct ContentCacheBudget {
    pub max_files: usize,
    pub cached_count: AtomicUsize,
    pub cached_bytes: AtomicU64,
}

impl ContentCacheBudget {
    /// No limits — every eligible file is cached. Useful for tests and
    /// short-lived tools that don't need resource protection.
    pub fn unlimited() -> Self {
        Self {
            max_files: usize::MAX,
            cached_count: AtomicUsize::new(0),
            cached_bytes: AtomicU64::new(0),
        }
    }

    pub fn zero() -> Self {
        Self {
            max_files: 0,
            cached_count: AtomicUsize::new(0),
            cached_bytes: AtomicU64::new(0),
        }
    }

    pub fn new(max_files: usize) -> Self {
        Self {
            max_files,
            cached_count: AtomicUsize::new(0),
            cached_bytes: AtomicU64::new(0),
        }
    }

    /// Reset the counters. Called when the file index is rebuilt (rescan /
    /// directory change) and all old `FileItem`s are dropped.
    pub fn reset(&self) {
        self.cached_count.store(0, Ordering::Relaxed);
        self.cached_bytes.store(0, Ordering::Relaxed);
    }
}

impl Default for ContentCacheBudget {
    fn default() -> Self {
        Self::new(30_000)
    }
}
