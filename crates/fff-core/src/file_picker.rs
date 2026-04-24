//! Core file picker: filesystem indexing, background watching, and fuzzy search.
//!
//! [`FilePicker`] is the central component of fff-search. It:
//!
//! 1. **Indexes** a directory tree in a background thread, collecting every
//!    non-ignored file into a path-sorted `Vec<FileItem>`.
//! 2. **Watches** the filesystem via the `notify` crate, applying
//!    create/modify/delete events to the index in real time.
//! 3. **Owns files**: Provides a values for search and provides a good entry point for
//!    fuzzy search and live grep
//!
//! # Lifecycle
//!
//! ```text
//!   new_with_shared_state()
//!     │
//!     ├─> background scan thread ──> populates SharedPicker
//!     └─> file-system watcher    ──> live updates SharedPicker
//!
//!   search()         <── borrows &self, delegates to fuzzy_search
//!   grep()           <── static, borrows &[FileItem] (live content search)
//!   trigger_rescan() <── synchronous re-index
//!   cancel()         <── shuts down background work
//! ```
//!
//! # Thread Safety
//!
//! `FilePicker` itself is **not** `Sync`!
//! all concurrent access goes through [`SharedPicker`](crate::SharedPicker) .
//! The background scanner and watcher acquire write locks only when mutating
//! the file index, so read-heavy search workloads rarely contend.

use crate::FFFStringStorage;
use crate::background_watcher::BackgroundWatcher;
use crate::bigram_filter::{BigramFilter, BigramIndexBuilder, BigramOverlay};
use crate::error::Error;
use crate::frecency::FrecencyTracker;
use crate::git::GitStatusCache;
use crate::grep::{GrepResult, GrepSearchOptions, grep_search, multi_grep_search};
use crate::ignore::non_git_repo_overrides;
use crate::query_tracker::QueryTracker;
use crate::score::fuzzy_match_and_score_files;
use crate::shared::{SharedFrecency, SharedPicker};
use crate::simd_path::ArenaPtr;
use crate::types::{
    ContentCacheBudget, DirItem, DirSearchResult, FileItem, MixedItemRef, MixedSearchResult,
    PaginationArgs, Score, ScoringContext, SearchResult,
};
use fff_query_parser::FFFQuery;
use git2::{Repository, Status, StatusOptions};
use rayon::prelude::*;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::time::SystemTime;
use tracing::{Level, debug, error, info, warn};

/// Dedicated thread pool for background work (scan, warmup, bigram build).
/// Uses fewer threads than the global rayon pool so Neovim's event loop
/// and search queries can still get CPU time.
static BACKGROUND_THREAD_POOL: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
    let total = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4);
    let bg_threads = total.saturating_sub(2).max(1);
    info!(
        "Background pool: {} threads (system has {})",
        bg_threads, total
    );
    rayon::ThreadPoolBuilder::new()
        .num_threads(bg_threads)
        .thread_name(|i| format!("fff-bg-{i}"))
        .build()
        .expect("failed to create background rayon pool")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FFFMode {
    #[default]
    Neovim,
    Ai,
}

impl FFFMode {
    pub fn is_ai(self) -> bool {
        self == FFFMode::Ai
    }
}

/// Configuration for a single fuzzy search invocation.
///
/// Passed to [`FilePicker::search`] to control threading, pagination,
/// and scoring behavior.
#[derive(Debug, Clone, Copy, Default)]
pub struct FuzzySearchOptions<'a> {
    pub max_threads: usize,
    pub current_file: Option<&'a str>,
    pub project_path: Option<&'a Path>,
    pub combo_boost_score_multiplier: i32,
    pub min_combo_count: u32,
    pub pagination: PaginationArgs,
}

#[derive(Debug, Clone)]
struct FileSync {
    git_workdir: Option<PathBuf>,
    /// Base files sorted by (parent_dir, filename). Used for binary search and
    /// bigram index. All paths are backed by `chunked_paths` arena.
    /// Deletions use tombstones (`is_deleted = true`) to keep bigram indices stable.
    files: Vec<FileItem>,
    /// Number of base files (from the last full reindex). Overflow files
    /// live at `files[base_count..]`, each with its own `ChunkedPathStore`
    /// kept alive in `overflow_stores`.
    base_count: usize,
    /// Sorted directory table. Each entry is a unique parent directory of at
    /// least one file in `files`. Sorted by absolute path for O(log n) lookup.
    /// Built during `walk_filesystem` and used for directory picker mode,
    /// per-directory stats, and as a fast replacement for `extract_watch_dirs`.
    dirs: Vec<DirItem>,
    /// Shared builder for overflow file paths. Each overflow file's ChunkedString
    /// uses `arena_override` pointing into this builder's arena. The builder
    /// grows incrementally — no per-file store allocation. Dropped on rescan.
    overflow_builder: Option<crate::simd_path::ChunkedPathStoreBuilder>,
    /// Compressed bigram inverted index built during the post-scan phase.
    /// Lives here so that replacing `FileSync` on rescan automatically drops
    /// the stale index (bigram file indices are positions in `files`).
    bigram_index: Option<Arc<BigramFilter>>,
    /// Overlay tracking file mutations since the bigram index was built.
    bigram_overlay: Option<Arc<parking_lot::RwLock<BigramOverlay>>>,
    /// Chunk-level deduped path store for zero-copy SIMD matching.
    /// Each file's relative path is pre-chunked into 16-byte aligned blocks
    /// with content-based deduplication across files.
    chunked_paths: Option<crate::simd_path::ChunkedPathStore>,
}

impl FileSync {
    fn new() -> Self {
        Self {
            files: Vec::new(),
            base_count: 0,
            dirs: Vec::new(),
            overflow_builder: None,
            git_workdir: None,
            bigram_index: None,
            bigram_overlay: None,
            chunked_paths: None,
        }
    }

    /// Arena for base files (from the last full scan).
    #[inline]
    fn arena_base_ptr(&self) -> ArenaPtr {
        self.chunked_paths
            .as_ref()
            .map(|s| s.as_arena_ptr())
            .unwrap_or(ArenaPtr::null())
    }

    /// Arena for overflow files (added after the last full scan).
    #[inline]
    fn overflow_arena_ptr(&self) -> ArenaPtr {
        self.overflow_builder
            .as_ref()
            .map(|b| b.as_arena_ptr())
            .unwrap_or(self.arena_base_ptr())
    }

    /// Resolve the correct arena for a given file (base vs overflow).
    #[inline]
    fn arena_for_file(&self, file: &FileItem) -> ArenaPtr {
        if file.is_overflow() {
            self.overflow_arena_ptr()
        } else {
            self.arena_base_ptr()
        }
    }

    /// Get all files (base + overflow). The base portion `[..base_count]` is
    /// sorted by path; the overflow tail is unsorted.
    #[inline]
    fn files(&self) -> &[FileItem] {
        &self.files
    }

    /// Get the overflow portion (files added since last full reindex).
    #[inline]
    fn overflow_files(&self) -> &[FileItem] {
        &self.files[self.base_count..]
    }

    /// Get mutable file at index (works for base files only).
    #[inline]
    fn get_file_mut(&mut self, index: usize) -> Option<&mut FileItem> {
        self.files.get_mut(index)
    }

    /// Find file index by path using binary search on the sorted base portion.
    /// `path` must be an absolute path under `base_path`.
    #[inline]
    fn find_file_index(&self, path: &Path, base_path: &Path) -> Result<usize, usize> {
        let arena = self.arena_base_ptr();

        // Strip base_path prefix to get the relative path.
        let rel_path = match path.strip_prefix(base_path) {
            Ok(r) => r.to_string_lossy(),
            Err(_) => return Err(0),
        };

        // Split into directory (with trailing '/') and filename.
        let parent_end = rel_path
            .rfind(std::path::is_separator)
            .map(|i| i + 1)
            .unwrap_or(0);
        let dir_rel = &rel_path[..parent_end];
        let filename = &rel_path[parent_end..];

        // Binary search dirs to find the parent directory index.
        // Dir items store the relative path including trailing '/' (e.g. "src/components/").
        let mut dir_buf = [0u8; crate::simd_path::PATH_BUF_SIZE];
        let dir_idx = match self
            .dirs
            .binary_search_by(|d| d.read_relative_path(arena, &mut dir_buf).cmp(dir_rel))
        {
            Ok(idx) => idx as u32,
            Err(_) => return Err(0), // directory not found
        };

        // Binary search files by (parent_dir, filename) — same order as the sort
        self.files[..self.base_count].binary_search_by(|f| {
            f.parent_dir_index().cmp(&dir_idx).then_with(|| {
                let fname = f.file_name(arena);
                fname.as_str().cmp(filename)
            })
        })
    }

    /// Find a file in the overflow portion by relative path (linear scan).
    /// Returns the absolute index into `files` (i.e. `base_count + position`).
    fn find_overflow_index(&self, rel_path: &str) -> Option<usize> {
        let overflow_arena = self.overflow_arena_ptr();
        self.files[self.base_count..]
            .iter()
            .position(|f| f.relative_path_eq(overflow_arena, rel_path))
            .map(|pos| self.base_count + pos)
    }

    /// Insert a file at position. Simple - no HashMap to maintain!
    fn insert_file(&mut self, position: usize, file: FileItem) {
        self.files.insert(position, file);
    }

    fn retain_files_with_arena<F>(&mut self, mut predicate: F) -> usize
    where
        F: FnMut(&FileItem, ArenaPtr) -> bool,
    {
        let base_arena = self.arena_base_ptr();
        let overflow_arena = self.overflow_arena_ptr();

        let base_count = self.base_count;
        let initial_len = self.files.len();
        let base_retained = self.files[..base_count]
            .iter()
            .filter(|f| predicate(f, base_arena))
            .count();

        self.files.retain(|f| {
            predicate(
                f,
                if f.is_overflow() {
                    overflow_arena
                } else {
                    base_arena
                },
            )
        });

        self.base_count = base_retained;
        initial_len - self.files.len()
    }

    /// Insert a file in sorted order (by path).
    /// Returns true if inserted, false if file already exists.
    fn insert_file_sorted(&mut self, file: FileItem, base_path: &Path) -> bool {
        let arena = self.arena_base_ptr();
        let abs_path = file.absolute_path(arena, base_path);
        match self.find_file_index(&abs_path, base_path) {
            Ok(_) => false, // File already exists
            Err(position) => {
                self.insert_file(position, file);
                true
            }
        }
    }
}

impl FileItem {
    pub fn new(path: PathBuf, base_path: &Path, git_status: Option<Status>) -> (Self, String) {
        let metadata = std::fs::metadata(&path).ok();
        Self::new_with_metadata(path, base_path, git_status, metadata.as_ref())
    }

    /// Create a FileItem using pre-fetched metadata to avoid a redundant stat syscall.
    /// Returns `(FileItem, relative_path)`. The FileItem's `path` field is
    /// empty; callers must populate it via `set_path` or `build_chunked_path_store_and_assign`.
    fn new_with_metadata(
        path: PathBuf,
        base_path: &Path,
        git_status: Option<Status>,
        metadata: Option<&std::fs::Metadata>,
    ) -> (Self, String) {
        let path_buf = pathdiff::diff_paths(&path, base_path).unwrap_or_else(|| path.clone());
        let relative_path = path_buf.to_string_lossy().into_owned();

        let (size, modified) = match metadata {
            Some(metadata) => {
                let size = metadata.len();
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());

                (size, modified)
            }
            None => (0, 0),
        };

        let is_binary = is_known_binary_extension(&path);

        let filename_start = relative_path
            .rfind(std::path::is_separator)
            .map(|i| i + 1)
            .unwrap_or(0) as u16;

        let item = Self::new_raw(filename_start, size, modified, git_status, is_binary);
        (item, relative_path)
    }

    /// Create a FileItem with an empty ChunkedString from a path on disk.
    ///
    /// Returns `(file_item, relative_path_string)`. The relative path must be
    /// kept alongside the FileItem until `build_chunked_path_store_and_assign`
    /// populates each item's `path` field from the shared arena.
    pub fn new_from_walk(
        path: &Path,
        base_path: &Path,
        git_status: Option<Status>,
        metadata: Option<&std::fs::Metadata>,
    ) -> (Self, String) {
        let (size, modified) = match metadata {
            Some(metadata) => {
                let size = metadata.len();
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                (size, modified)
            }
            None => (0, 0),
        };

        let is_binary = is_known_binary_extension(path);

        let rel = pathdiff::diff_paths(path, base_path).unwrap_or_else(|| path.to_path_buf());
        let rel_str = rel.to_string_lossy().into_owned();
        let fname_offset = rel_str
            .rfind(std::path::is_separator)
            .map(|i| i + 1)
            .unwrap_or(0) as u16;

        let item = Self::new_raw(fname_offset, size, modified, git_status, is_binary);
        (item, rel_str)
    }

    pub(crate) fn update_frecency_scores(
        &mut self,
        tracker: &FrecencyTracker,
        arena: ArenaPtr,
        base_path: &Path,
        mode: FFFMode,
    ) -> Result<(), Error> {
        let mut abs_buf = [0u8; crate::simd_path::PATH_BUF_SIZE];
        let abs = self.write_absolute_path(arena, base_path, &mut abs_buf);
        self.access_frecency_score = tracker.get_access_score(abs, mode) as i16;
        self.modification_frecency_score =
            tracker.get_modification_score(self.modified, self.git_status, mode) as i16;

        Ok(())
    }
}

/// Options for creating a [`FilePicker`].
pub struct FilePickerOptions {
    pub base_path: String,
    /// Pre-populate mmap caches for top-frecency files after the initial scan.
    pub enable_mmap_cache: bool,
    /// Build content index after the initial scan for faster content-aware filtering.
    pub enable_content_indexing: bool,
    /// Mode of the picker impact the way file watcher events are handled and the scoring logic
    pub mode: FFFMode,
    /// Explicit cache budget. When `None`, the budget is auto-computed from
    /// the repo size after the initial scan completes.
    pub cache_budget: Option<ContentCacheBudget>,
    /// When `false`, `new_with_shared_state` skips the background file watcher.
    pub watch: bool,
}

impl Default for FilePickerOptions {
    fn default() -> Self {
        Self {
            base_path: ".".into(),
            enable_mmap_cache: false,
            enable_content_indexing: false,
            mode: FFFMode::default(),
            cache_budget: None,
            watch: true,
        }
    }
}

pub struct FilePicker {
    pub mode: FFFMode,
    pub base_path: PathBuf,
    pub is_scanning: Arc<AtomicBool>,
    sync_data: FileSync,
    cache_budget: Arc<ContentCacheBudget>,
    has_explicit_cache_budget: bool,
    watcher_ready: Arc<AtomicBool>,
    scanned_files_count: Arc<AtomicUsize>,
    background_watcher: Option<BackgroundWatcher>,
    enable_mmap_cache: bool,
    enable_content_indexing: bool,
    watch: bool,
    cancelled: Arc<AtomicBool>,
    // This is a soft lock that we use to prevent rescan be triggered while the
    // bigram indexing is in progress. This allows to keep some of the unsafe magic
    // relying on the immutabillity of the files vec after the index without worrying
    // that the vec is going to be dropped before the indexing is finished
    //
    // In addition to that rescan is likely triggered by something unnecessary
    // before the indexing is finished it means that fff is dogfooded the index either
    // by the UI rendering preview or simply by walking the directory. Which is not good anyway
    post_scan_busy: Arc<AtomicBool>,
}

impl std::fmt::Debug for FilePicker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilePicker")
            .field("base_path", &self.base_path)
            .field("sync_data", &self.sync_data)
            .field("is_scanning", &self.is_scanning.load(Ordering::Relaxed))
            .field(
                "scanned_files_count",
                &self.scanned_files_count.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

impl FFFStringStorage for &FilePicker {
    #[inline]
    fn arena_for(&self, file: &FileItem) -> crate::simd_path::ArenaPtr {
        self.sync_data.arena_for_file(file)
    }

    #[inline]
    fn base_arena(&self) -> crate::simd_path::ArenaPtr {
        self.sync_data.arena_base_ptr()
    }

    #[inline]
    fn overflow_arena(&self) -> crate::simd_path::ArenaPtr {
        self.sync_data.overflow_arena_ptr()
    }
}

impl FilePicker {
    pub fn base_path(&self) -> &Path {
        &self.base_path
    }

    /// Convert an absolute path to a relative path string (relative to base_path).
    /// Returns None if the path doesn't start with base_path.
    fn to_relative_path<'a>(&self, path: &'a Path) -> Option<&'a str> {
        path.strip_prefix(&self.base_path)
            .ok()
            .and_then(|p| p.to_str())
    }

    pub fn need_enable_mmap_cache(&self) -> bool {
        self.enable_mmap_cache
    }

    pub fn need_enable_content_indexing(&self) -> bool {
        self.enable_content_indexing
    }

    pub fn need_watch(&self) -> bool {
        self.watch
    }

    pub fn mode(&self) -> FFFMode {
        self.mode
    }

    pub fn cache_budget(&self) -> &ContentCacheBudget {
        &self.cache_budget
    }

    pub fn bigram_index(&self) -> Option<&BigramFilter> {
        self.sync_data.bigram_index.as_deref()
    }

    pub fn bigram_overlay(&self) -> Option<&parking_lot::RwLock<BigramOverlay>> {
        self.sync_data.bigram_overlay.as_deref()
    }

    pub fn get_file_mut(&mut self, index: usize) -> Option<&mut FileItem> {
        self.sync_data.get_file_mut(index)
    }

    pub fn set_bigram_index(&mut self, index: BigramFilter, overlay: BigramOverlay) {
        self.sync_data.bigram_index = Some(Arc::new(index));
        self.sync_data.bigram_overlay = Some(Arc::new(parking_lot::RwLock::new(overlay)));
    }

    pub fn git_root(&self) -> Option<&Path> {
        self.sync_data.git_workdir.as_deref()
    }

    /// Get all indexed files sorted by path.
    /// Note: Files are stored sorted by PATH for efficient insert/remove.
    /// For frecency-sorted results, use search() which sorts matched results.
    pub fn get_files(&self) -> &[FileItem] {
        self.sync_data.files()
    }

    pub fn get_overflow_files(&self) -> &[FileItem] {
        self.sync_data.overflow_files()
    }

    /// Get the directory table (sorted by path).
    pub fn get_dirs(&self) -> &[DirItem] {
        &self.sync_data.dirs
    }

    /// Actual heap bytes used: (chunked_path_store, 0, 0).
    /// The second element is 0 because leaked overflow stores aren't tracked.
    pub fn arena_bytes(&self) -> (usize, usize, usize) {
        let chunked = self
            .sync_data
            .chunked_paths
            .as_ref()
            .map_or(0, |s| s.heap_bytes());
        (chunked, 0, 0)
    }

    /// Extracts all unique ancestor directories from the indexed file list.
    /// Uses the pre-built directory table when available (O(d) where d = unique dirs),
    /// falling back to the old traversal for overflow files.
    #[tracing::instrument(level = "debug", skip(self))]
    pub fn extract_watch_dirs(&self) -> Vec<PathBuf> {
        let dir_table = &self.sync_data.dirs;

        if !dir_table.is_empty() {
            // Fast path: just collect PathBufs from the dir table.
            // The dir table already contains all unique parent directories.
            // We also need ancestor directories (parents of parents) for the
            // watcher to work. Walk up from each dir to the base.
            let base = self.base_path.as_path();
            let arena = self.arena_base_ptr();
            let mut all_dirs = Vec::with_capacity(dir_table.len() * 2);
            let mut seen = std::collections::HashSet::with_capacity(dir_table.len() * 2);

            for dir_item in dir_table {
                let mut current = dir_item.absolute_path(arena, base);
                while current.as_path() != base {
                    if !seen.insert(current.clone()) {
                        break; // already visited this and all its ancestors
                    }
                    all_dirs.push(current.clone());
                    if !current.pop() {
                        break;
                    }
                }
            }

            return all_dirs;
        }

        // Fallback: old traversal for cases where dir table is empty
        let files = self.sync_data.files();
        let base = self.base_path.as_path();
        let arena = self.arena_base_ptr();
        let mut dirs = Vec::with_capacity(files.len() / 4);
        let mut current = self.base_path.clone();

        for file in files {
            let abs = file.absolute_path(arena, base);
            let Some(parent) = abs.parent() else {
                continue;
            };
            if parent == current.as_path() {
                continue;
            }

            while current.as_path() != base && !parent.starts_with(&current) {
                current.pop();
            }

            let Ok(remainder) = parent.strip_prefix(&current) else {
                continue;
            };
            for component in remainder.components() {
                current.push(component);
                dirs.push(current.clone());
            }
        }

        dirs
    }

    /// Create a new FilePicker from options.
    /// Always prefer new_with_shared_state for the consumer application, use this only if you know
    /// what you are doing. This won't spawn the backgraound watcher and won't walk the file tree.
    pub fn new(options: FilePickerOptions) -> Result<Self, Error> {
        let path = PathBuf::from(&options.base_path);
        if !path.exists() {
            error!("Base path does not exist: {}", options.base_path);
            return Err(Error::InvalidPath(path));
        }
        if path.parent().is_none() {
            error!("Refusing to index filesystem root: {}", path.display());
            return Err(Error::FilesystemRoot(path));
        }

        let has_explicit_budget = options.cache_budget.is_some();
        let initial_budget = options.cache_budget.unwrap_or_default();

        Ok(FilePicker {
            background_watcher: None,
            base_path: path,
            cache_budget: Arc::new(initial_budget),
            cancelled: Arc::new(AtomicBool::new(false)),
            has_explicit_cache_budget: has_explicit_budget,
            is_scanning: Arc::new(AtomicBool::new(false)),
            mode: options.mode,
            post_scan_busy: Arc::new(AtomicBool::new(false)),
            scanned_files_count: Arc::new(AtomicUsize::new(0)),
            sync_data: FileSync::new(),
            enable_mmap_cache: options.enable_mmap_cache,
            enable_content_indexing: options.enable_content_indexing,
            watch: options.watch,
            watcher_ready: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Create a picker, place it into the shared handle, and spawn background
    /// indexing + file-system watcher. This is the default entry point.
    pub fn new_with_shared_state(
        shared_picker: SharedPicker,
        shared_frecency: SharedFrecency,
        options: FilePickerOptions,
    ) -> Result<(), Error> {
        let picker = Self::new(options)?;

        info!(
            "Spawning background threads: base_path={}, warmup={}, content_indexing={}, mode={:?}",
            picker.base_path.display(),
            picker.enable_mmap_cache,
            picker.enable_content_indexing,
            picker.mode,
        );

        let warmup = picker.enable_mmap_cache;
        let content_indexing = picker.enable_content_indexing;
        let watch = picker.watch;
        let mode = picker.mode;

        picker.is_scanning.store(true, Ordering::Release);

        let scan_signal = Arc::clone(&picker.is_scanning);
        let watcher_ready = Arc::clone(&picker.watcher_ready);
        let synced_files_count = Arc::clone(&picker.scanned_files_count);
        let cancelled = Arc::clone(&picker.cancelled);
        let post_scan_busy = Arc::clone(&picker.post_scan_busy);
        let path = picker.base_path.clone();

        {
            let mut guard = shared_picker.write()?;
            *guard = Some(picker);
        }

        spawn_scan_and_watcher(
            path,
            scan_signal,
            watcher_ready,
            synced_files_count,
            warmup,
            content_indexing,
            watch,
            mode,
            shared_picker,
            shared_frecency,
            cancelled,
            post_scan_busy,
        );

        Ok(())
    }

    /// Synchronous filesystem scan — populates `self` with indexed files.
    ///
    /// Use this when you need direct access to the picker without shared state:
    /// ```ignore
    /// let mut picker = FilePicker::new(options)?;
    /// picker.collect_files()?;
    /// // picker.get_files() is now populated
    /// ```
    pub fn collect_files(&mut self) -> Result<(), Error> {
        self.is_scanning.store(true, Ordering::Relaxed);
        self.scanned_files_count.store(0, Ordering::Relaxed);

        let empty_frecency = SharedFrecency::default();
        let walk = walk_filesystem(
            &self.base_path,
            &self.scanned_files_count,
            &empty_frecency,
            self.mode,
        )?;

        self.sync_data = walk.sync;

        // Recalculate cache budget based on actual file count (unless
        // the caller provided an explicit budget via FilePickerOptions).
        if !self.has_explicit_cache_budget {
            let file_count = self.sync_data.files().len();
            self.cache_budget = Arc::new(ContentCacheBudget::new_for_repo(file_count));
        } else {
            self.cache_budget.reset();
        }

        // Apply git status synchronously.
        if let Ok(Some(git_cache)) = walk.git_handle.join() {
            let arena = self.arena_base_ptr();
            for file in self.sync_data.files.iter_mut() {
                file.git_status =
                    git_cache.lookup_status(&file.absolute_path(arena, &self.base_path));
            }
        }

        self.is_scanning.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Start the background file-system watcher.
    ///
    /// The picker must already be placed into `shared_picker` (the watcher
    /// needs the shared handle to apply live updates). Call after
    /// [`collect_files`](Self::collect_files) or after an initial scan.
    pub fn spawn_background_watcher(
        &mut self,
        shared_picker: &SharedPicker,
        shared_frecency: &SharedFrecency,
    ) -> Result<(), Error> {
        let git_workdir = self.sync_data.git_workdir.clone();
        let watch_dirs = self.extract_watch_dirs();
        let watcher = BackgroundWatcher::new(
            self.base_path.clone(),
            git_workdir,
            shared_picker.clone(),
            shared_frecency.clone(),
            self.mode,
            watch_dirs,
        )?;
        self.background_watcher = Some(watcher);
        self.watcher_ready.store(true, Ordering::Release);
        Ok(())
    }

    /// Perform fuzzy search on files with a pre-parsed query.
    ///
    /// The query should be parsed using [`FFFQuery`]::parse() before calling
    /// this function. If a [`QueryTracker`] is provided, the search will
    /// automatically look up the last selected file for this query and apply
    /// combo-boost scoring.
    ///
    pub fn fuzzy_search<'q>(
        &self,
        query: &'q FFFQuery<'q>,
        query_tracker: Option<&QueryTracker>,
        options: FuzzySearchOptions<'q>,
    ) -> SearchResult<'_> {
        let files = self.get_files();
        let max_threads = if options.max_threads == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        } else {
            options.max_threads
        };

        debug!(
            raw_query = ?query.raw_query,
            pagination = ?options.pagination,
            ?max_threads,
            current_file = ?options.current_file,
            "Fuzzy search",
        );

        let total_files = files.len();
        let location = query.location;

        // Get effective query for max_typos calculation (without location suffix)
        let effective_query = match &query.fuzzy_query {
            fff_query_parser::FuzzyQuery::Text(t) => *t,
            fff_query_parser::FuzzyQuery::Parts(parts) if !parts.is_empty() => parts[0],
            _ => query.raw_query.trim(),
        };

        // small queries with a large number of results can match absolutely everything
        let max_typos = (effective_query.len() as u16 / 4).clamp(2, 6);
        // Look up the last file selected for this query (combo-boost scoring)
        let last_same_query_entry =
            query_tracker
                .zip(options.project_path)
                .and_then(|(tracker, project_path)| {
                    tracker
                        .get_last_query_entry(
                            query.raw_query,
                            project_path,
                            options.min_combo_count,
                        )
                        .ok()
                        .flatten()
                });

        let context = ScoringContext {
            query,
            max_typos,
            max_threads,
            project_path: options.project_path,
            current_file: options.current_file,
            last_same_query_match: last_same_query_entry,
            combo_boost_score_multiplier: options.combo_boost_score_multiplier,
            min_combo_count: options.min_combo_count,
            pagination: options.pagination,
        };

        let time = std::time::Instant::now();

        let base_arena = self.sync_data.arena_base_ptr();
        let overflow_arena = self
            .sync_data
            .overflow_builder
            .as_ref()
            .map(|b| b.as_arena_ptr())
            .unwrap_or(base_arena);

        let (items, scores, total_matched) = fuzzy_match_and_score_files(
            files,
            &context,
            self.sync_data.base_count,
            base_arena,
            overflow_arena,
        );

        info!(
            ?query,
            completed_in = ?time.elapsed(),
            total_matched,
            returned_count = items.len(),
            pagination = ?options.pagination,
            "Fuzzy search completed",
        );

        SearchResult {
            items,
            scores,
            total_matched,
            total_files,
            location,
        }
    }

    /// Perform fuzzy search on indexed directories.
    ///
    /// Returns directories ranked by fuzzy match quality + frecency.
    pub fn fuzzy_search_directories<'q>(
        &self,
        query: &'q FFFQuery<'q>,
        options: FuzzySearchOptions<'q>,
    ) -> DirSearchResult<'_> {
        let dirs = self.get_dirs();
        let max_threads = if options.max_threads == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        } else {
            options.max_threads
        };

        let total_dirs = dirs.len();

        let effective_query = match &query.fuzzy_query {
            fff_query_parser::FuzzyQuery::Text(t) => *t,
            fff_query_parser::FuzzyQuery::Parts(parts) if !parts.is_empty() => parts[0],
            _ => query.raw_query.trim(),
        };

        let max_typos = (effective_query.len() as u16 / 4).clamp(2, 6);

        let context = ScoringContext {
            query,
            max_typos,
            max_threads,
            project_path: options.project_path,
            current_file: options.current_file,
            last_same_query_match: None,
            combo_boost_score_multiplier: 0,
            min_combo_count: 0,
            pagination: options.pagination,
        };

        let arena = self.sync_data.arena_base_ptr();
        let time = std::time::Instant::now();

        let (items, scores, total_matched) =
            crate::score::fuzzy_match_and_score_dirs(dirs, &context, arena);

        info!(
            ?query,
            completed_in = ?time.elapsed(),
            total_matched,
            returned_count = items.len(),
            "Directory search completed",
        );

        DirSearchResult {
            items,
            scores,
            total_matched,
            total_dirs,
        }
    }

    /// Perform a mixed fuzzy search across both files and directories.
    ///
    /// Returns a single flat list where files and directories are interleaved
    /// by total score in descending order.
    ///
    /// If the raw query ends with a path separator (`/`), only directories
    /// are searched — files are skipped entirely. The caller should parse the
    /// query with `DirSearchConfig` so that trailing `/` is kept as fuzzy
    /// text instead of becoming a `PathSegment` constraint.
    pub fn fuzzy_search_mixed<'q>(
        &self,
        query: &'q FFFQuery<'q>,
        query_tracker: Option<&QueryTracker>,
        options: FuzzySearchOptions<'q>,
    ) -> MixedSearchResult<'_> {
        let location = query.location;
        let page_offset = options.pagination.offset;
        let page_limit = if options.pagination.limit > 0 {
            options.pagination.limit
        } else {
            100
        };

        let dirs_only =
            query.raw_query.ends_with(std::path::MAIN_SEPARATOR) || query.raw_query.ends_with('/');

        // Run file search and dir search with no pagination (we merge then paginate).
        let internal_limit = page_offset.saturating_add(page_limit).saturating_mul(2);

        let dir_options = FuzzySearchOptions {
            pagination: PaginationArgs {
                offset: 0,
                limit: internal_limit,
            },
            ..options
        };
        let dir_results = self.fuzzy_search_directories(query, dir_options);

        if dirs_only {
            let total_matched = dir_results.total_matched;
            let total_dirs = dir_results.total_dirs;

            let mut merged: Vec<(MixedItemRef<'_>, Score)> =
                Vec::with_capacity(dir_results.items.len());
            for (dir, score) in dir_results.items.into_iter().zip(dir_results.scores) {
                merged.push((MixedItemRef::Dir(dir), score));
            }

            if page_offset >= merged.len() {
                return MixedSearchResult {
                    items: vec![],
                    scores: vec![],
                    total_matched,
                    total_files: self.sync_data.files().len(),
                    total_dirs,
                    location,
                };
            }

            let end = (page_offset + page_limit).min(merged.len());
            let page = merged.drain(page_offset..end);
            let (items, scores): (Vec<_>, Vec<_>) = page.unzip();

            return MixedSearchResult {
                items,
                scores,
                total_matched,
                total_files: self.sync_data.files().len(),
                total_dirs,
                location,
            };
        }

        let file_options = FuzzySearchOptions {
            pagination: PaginationArgs {
                offset: 0,
                limit: internal_limit,
            },
            ..options
        };
        let file_results = self.fuzzy_search(query, query_tracker, file_options);

        // Merge by score descending.
        let total_matched = file_results.total_matched + dir_results.total_matched;
        let total_files = file_results.total_files;
        let total_dirs = dir_results.total_dirs;

        let mut merged: Vec<(MixedItemRef<'_>, Score)> =
            Vec::with_capacity(file_results.items.len() + dir_results.items.len());

        for (file, score) in file_results.items.into_iter().zip(file_results.scores) {
            merged.push((MixedItemRef::File(file), score));
        }
        for (dir, score) in dir_results.items.into_iter().zip(dir_results.scores) {
            merged.push((MixedItemRef::Dir(dir), score));
        }

        // Sort merged results by total score descending.
        merged.sort_unstable_by_key(|b| std::cmp::Reverse(b.1.total));

        // Paginate.
        if page_offset >= merged.len() {
            return MixedSearchResult {
                items: vec![],
                scores: vec![],
                total_matched,
                total_files,
                total_dirs,
                location,
            };
        }

        let end = (page_offset + page_limit).min(merged.len());
        let page = merged.drain(page_offset..end);
        let (items, scores): (Vec<_>, Vec<_>) = page.unzip();

        MixedSearchResult {
            items,
            scores,
            total_matched,
            total_files,
            total_dirs,
            location,
        }
    }

    /// Perform a live grep search across indexed files.
    ///
    /// If `options.abort_signal` is set it overrides the picker's internal
    /// cancellation flag, giving the caller full control over when to stop.
    pub fn grep(&self, query: &FFFQuery<'_>, options: &GrepSearchOptions) -> GrepResult<'_> {
        let overlay_guard = self.sync_data.bigram_overlay.as_ref().map(|o| o.read());
        let arena = self.arena_base_ptr();
        let overflow_arena = self.sync_data.overflow_arena_ptr();
        let cancel = options.abort_signal.as_deref().unwrap_or(&self.cancelled);

        grep_search(
            self.get_files(),
            query,
            options,
            self.cache_budget(),
            self.sync_data.bigram_index.as_deref(),
            overlay_guard.as_deref(),
            cancel,
            &self.base_path,
            arena,
            overflow_arena,
        )
    }

    /// Multi-pattern grep search across indexed files.
    pub fn multi_grep(
        &self,
        patterns: &[&str],
        constraints: &[fff_query_parser::Constraint<'_>],
        options: &GrepSearchOptions,
    ) -> GrepResult<'_> {
        let overlay_guard = self.sync_data.bigram_overlay.as_ref().map(|o| o.read());
        let arena = self.arena_base_ptr();
        let overflow_arena = self.sync_data.overflow_arena_ptr();
        let cancel = options.abort_signal.as_deref().unwrap_or(&self.cancelled);

        multi_grep_search(
            self.get_files(),
            patterns,
            constraints,
            options,
            self.cache_budget(),
            self.sync_data.bigram_index.as_deref(),
            overlay_guard.as_deref(),
            cancel,
            &self.base_path,
            arena,
            overflow_arena,
        )
    }

    /// Like [`grep`](Self::grep) but ignores the bigram overlay.
    pub fn grep_without_overlay(
        &self,
        query: &FFFQuery<'_>,
        options: &GrepSearchOptions,
    ) -> GrepResult<'_> {
        let arena = self.arena_base_ptr();
        let overflow_arena = self.sync_data.overflow_arena_ptr();
        let cancel = options.abort_signal.as_deref().unwrap_or(&self.cancelled);

        grep_search(
            self.get_files(),
            query,
            options,
            self.cache_budget(),
            self.sync_data.bigram_index.as_deref(),
            None,
            cancel,
            &self.base_path,
            arena,
            overflow_arena,
        )
    }

    // Returns an ongoing or finisshed scan progress
    pub fn get_scan_progress(&self) -> ScanProgress {
        let scanned_count = self.scanned_files_count.load(Ordering::Relaxed);
        let is_scanning = self.is_scanning.load(Ordering::Relaxed);
        ScanProgress {
            scanned_files_count: scanned_count,
            is_scanning,
            is_watcher_ready: self.watcher_ready.load(Ordering::Relaxed),
            is_warmup_complete: self.sync_data.bigram_index.is_some(),
        }
    }

    /// Update git statuses for files, using the provided shared frecency tracker.
    pub fn update_git_statuses(
        &mut self,
        status_cache: GitStatusCache,
        shared_frecency: &SharedFrecency,
    ) -> Result<(), Error> {
        debug!(
            statuses_count = status_cache.statuses_len(),
            "Updating git status",
        );

        let mode = self.mode;
        let bp = self.base_path.clone();
        let arena = self.arena_base_ptr();
        let frecency = shared_frecency.read()?;
        status_cache
            .into_iter()
            .try_for_each(|(path, status)| -> Result<(), Error> {
                if let Some(file) = self.get_mut_file_by_path(&path) {
                    file.git_status = Some(status);
                    if let Some(ref f) = *frecency {
                        file.update_frecency_scores(f, arena, &bp, mode)?;
                    }
                    // Update parent dir frecency inline.
                    let score = file.access_frecency_score as i32;
                    let dir_idx = file.parent_dir_index() as usize;
                    if let Some(dir) = self.sync_data.dirs.get_mut(dir_idx) {
                        dir.update_frecency_if_larger(score);
                    }
                } else {
                    error!(?path, "Couldn't update the git status for path");
                }
                Ok(())
            })?;

        Ok(())
    }

    pub fn update_single_file_frecency(
        &mut self,
        file_path: impl AsRef<Path>,
        frecency_tracker: &FrecencyTracker,
    ) -> Result<(), Error> {
        let path = file_path.as_ref();
        let arena = self.arena_base_ptr();
        let rel = self.to_relative_path(path).unwrap_or("");
        let index = self
            .sync_data
            .find_file_index(path, &self.base_path)
            .ok()
            .or_else(|| self.sync_data.find_overflow_index(rel));
        if let Some(index) = index
            && let Some(file) = self.sync_data.get_file_mut(index)
        {
            file.update_frecency_scores(frecency_tracker, arena, &self.base_path, self.mode)?;

            // Update parent dir frecency inline (only if larger).
            let score = file.access_frecency_score as i32;
            let dir_idx = file.parent_dir_index() as usize;
            if let Some(dir) = self.sync_data.dirs.get_mut(dir_idx) {
                dir.update_frecency_if_larger(score);
            }
        }

        Ok(())
    }

    pub fn get_file_by_path(&self, path: impl AsRef<Path>) -> Option<&FileItem> {
        self.sync_data
            .find_file_index(path.as_ref(), &self.base_path)
            .ok()
            .and_then(|index| self.sync_data.files().get(index))
    }

    pub fn get_mut_file_by_path(&mut self, path: impl AsRef<Path>) -> Option<&mut FileItem> {
        let path = path.as_ref();
        let rel = self.to_relative_path(path).unwrap_or("");
        let index = self
            .sync_data
            .find_file_index(path, &self.base_path)
            .ok()
            .or_else(|| self.sync_data.find_overflow_index(rel));
        index.and_then(|i| self.sync_data.get_file_mut(i))
    }

    /// Add a file to the picker's files in sorted order (used by background watcher)
    pub fn add_file_sorted(&mut self, file: FileItem) -> Option<&FileItem> {
        let arena = self.arena_base_ptr();
        let path = file.absolute_path(arena, &self.base_path);

        if self.sync_data.insert_file_sorted(file, &self.base_path) {
            // File was inserted, look it up
            self.sync_data
                .find_file_index(&path, &self.base_path)
                .ok()
                .and_then(|idx| self.sync_data.get_file_mut(idx))
                .map(|file_mut| &*file_mut) // Convert &mut to &
        } else {
            // File already exists
            warn!(
                "Trying to insert a file that already exists: {}",
                path.display()
            );
            self.sync_data
                .find_file_index(&path, &self.base_path)
                .ok()
                .and_then(|idx| self.sync_data.get_file_mut(idx))
                .map(|file_mut| &*file_mut) // Convert &mut to &
        }
    }

    #[tracing::instrument(skip(self), name = "timing_update", level = Level::DEBUG)]
    pub fn on_create_or_modify(&mut self, path: impl AsRef<Path> + Debug) -> Option<&FileItem> {
        let path = path.as_ref();
        let overlay = self.sync_data.bigram_overlay.as_ref().map(Arc::clone);

        if let Ok(pos) = self.sync_data.find_file_index(path, &self.base_path) {
            let file = self.sync_data.get_file_mut(pos)?;

            if file.is_deleted() {
                // Resurrect tombstoned file.
                file.set_deleted(false);
                debug!(
                    "on_create_or_modify: resurrected tombstoned file at index {}",
                    pos
                );
            }

            debug!(
                "on_create_or_modify: file EXISTS at index {}, updating metadata",
                pos
            );

            let modified = match std::fs::metadata(path) {
                Ok(metadata) => metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()),
                Err(e) => {
                    error!("Failed to get metadata for {}: {}", path.display(), e);
                    None
                }
            };

            if let Some(modified) = modified {
                let modified = modified.as_secs();
                if file.modified < modified {
                    file.modified = modified;
                    file.invalidate_mmap(&self.cache_budget);
                }
            }

            // Update the bigram overlay for this modified file.
            if let Some(ref overlay) = overlay
                && let Ok(content) = std::fs::read(path)
            {
                overlay.write().modify_file(pos, &content);
            }

            return Some(&*file);
        }

        // Check overflow for existing added files.
        let rel_path = self.to_relative_path(path).unwrap_or("");
        if let Some(abs_idx) = self.sync_data.find_overflow_index(rel_path) {
            let file = self.sync_data.get_file_mut(abs_idx)?;
            let modified = std::fs::metadata(path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok());
            if let Some(modified) = modified {
                let modified = modified.as_secs();
                if file.modified < modified {
                    file.modified = modified;
                    file.invalidate_mmap(&self.cache_budget);
                }
            }
            return Some(&*file);
        }

        // New file — append to overflow (preserves base indices for bigram).
        debug!(
            "on_create_or_modify: file NEW, appending to overflow (base: {}, overflow: {})",
            self.sync_data.base_count,
            self.sync_data.overflow_files().len(),
        );

        let (mut file_item, rel_path) = FileItem::new(path.to_path_buf(), &self.base_path, None);

        // Lazily create the shared overflow builder on first use.
        let builder = self
            .sync_data
            .overflow_builder
            .get_or_insert_with(|| crate::simd_path::ChunkedPathStoreBuilder::new(64));

        let cs = builder.add_file_immediate(&rel_path, file_item.path.filename_offset);
        file_item.set_path(cs);
        file_item.set_overflow(true);
        self.sync_data.files.push(file_item);
        self.sync_data.files.last()
    }

    /// Tombstone a file instead of removing it, keeping base indices stable.
    pub fn remove_file_by_path(&mut self, path: impl AsRef<Path>) -> bool {
        let path = path.as_ref();
        match self.sync_data.find_file_index(path, &self.base_path) {
            Ok(index) => {
                let file = &mut self.sync_data.files[index];
                file.set_deleted(true);
                file.invalidate_mmap(&self.cache_budget);
                if let Some(ref overlay) = self.sync_data.bigram_overlay {
                    overlay.write().delete_file(index);
                }
                true
            }
            Err(_) => {
                // Check overflow for added files — these can be removed directly
                // since they aren't in the base bigram index.
                let rel = self.to_relative_path(path).unwrap_or("");
                if let Some(abs_pos) = self.sync_data.find_overflow_index(rel) {
                    self.sync_data.files.remove(abs_pos);
                    true
                } else {
                    false
                }
            }
        }
    }

    // TODO make this O(n)
    pub fn remove_all_files_in_dir(&mut self, dir: impl AsRef<Path>) -> usize {
        let dir_path = dir.as_ref();
        let relative_dir = self.to_relative_path(dir_path).unwrap_or("").to_string();

        let dir_prefix = if relative_dir.is_empty() {
            String::new()
        } else {
            format!("{}{}", relative_dir, std::path::MAIN_SEPARATOR)
        };

        self.sync_data.retain_files_with_arena(|file, arena| {
            !file.relative_path_starts_with(arena, &dir_prefix)
        })
    }

    /// Use this to prevent any substantial background threads from acquiring the locks
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn stop_background_monitor(&mut self) {
        if let Some(mut watcher) = self.background_watcher.take() {
            watcher.stop();
        }
    }

    #[inline]
    pub(crate) fn arena_base_ptr(&self) -> ArenaPtr {
        self.sync_data.arena_base_ptr()
    }

    /// Spawn a background thread to rebuild the bigram index after rescan.
    pub(crate) fn spawn_post_rescan_rebuild(&self, shared_picker: SharedPicker) -> bool {
        if self.cancelled.load(Ordering::Relaxed) {
            return false;
        }

        let post_scan_busy = Arc::clone(&self.post_scan_busy);
        let cancelled = Arc::clone(&self.cancelled);
        let auto_budget = !self.has_explicit_cache_budget;
        let do_warmup = self.enable_mmap_cache;
        let do_content_indexing = self.enable_content_indexing;

        post_scan_busy.store(true, Ordering::Release);

        std::thread::spawn(move || {
            let phase_start = std::time::Instant::now();

            // Scale cache budget if not explicitly configured.
            if auto_budget
                && !cancelled.load(Ordering::Acquire)
                && let Ok(mut guard) = shared_picker.write()
                && let Some(ref mut picker) = *guard
                && !picker.has_explicit_cache_budget
            {
                let file_count = picker.sync_data.files().len();
                picker.cache_budget = Arc::new(ContentCacheBudget::new_for_repo(file_count));
            }

            // Take a snapshot of files + budget while holding a brief read lock.
            // SAFETY: post_scan_busy blocks trigger_rescan from replacing
            // sync_data, so the Vec backing this slice stays alive.
            let files_snapshot = if !cancelled.load(Ordering::Acquire) {
                shared_picker.read().ok().and_then(|guard| {
                    guard.as_ref().map(|picker| {
                        let files = picker.sync_data.files();
                        let ptr = files.as_ptr();
                        let len = files.len();
                        let base_count = picker.sync_data.base_count;
                        let budget = Arc::clone(&picker.cache_budget);
                        let static_files: &[FileItem] =
                            unsafe { std::slice::from_raw_parts(ptr, len) };
                        (
                            static_files,
                            base_count,
                            budget,
                            picker.base_path().to_path_buf(),
                            picker.arena_base_ptr(),
                        )
                    })
                })
            } else {
                None
            };

            if let Some((files, base_count, budget, bp, arena)) = files_snapshot {
                // Warmup mmap caches.
                if do_warmup && !cancelled.load(Ordering::Acquire) {
                    let t = std::time::Instant::now();
                    warmup_mmaps(files, &budget, &bp, arena);
                    info!(
                        "Rescan warmup completed in {:.2}s (cached {} files, {} bytes)",
                        t.elapsed().as_secs_f64(),
                        budget.cached_count.load(Ordering::Relaxed),
                        budget.cached_bytes.load(Ordering::Relaxed),
                    );
                }

                // Build bigram index (lock-free).
                if do_content_indexing && !cancelled.load(Ordering::Acquire) {
                    let t = std::time::Instant::now();
                    // Index ONLY base files — overflow files are searched
                    // unconditionally by the grep overflow loop, so
                    // `BigramFilter::file_count` must equal
                    // `BigramOverlay::base_file_count` for the candidate
                    // bitset to never carry overflow-range bits.
                    let base_files = &files[..base_count.min(files.len())];
                    info!(
                        "Rescan: starting bigram index build for {} files...",
                        base_files.len()
                    );
                    let (index, content_binary) =
                        build_bigram_index(base_files, &budget, &bp, arena);
                    info!(
                        "Rescan: bigram index ready in {:.2}s",
                        t.elapsed().as_secs_f64()
                    );

                    // Brief write lock to store the index.
                    if let Ok(mut guard) = shared_picker.write()
                        && let Some(ref mut picker) = *guard
                    {
                        for &idx in &content_binary {
                            if let Some(file) = picker.sync_data.get_file_mut(idx) {
                                file.set_binary(true);
                            }
                        }

                        // Use the same `base_count` the filter was built with
                        // so `file_count == base_file_count` is guaranteed.
                        picker.sync_data.bigram_index = Some(Arc::new(index));
                        picker.sync_data.bigram_overlay = Some(Arc::new(parking_lot::RwLock::new(
                            BigramOverlay::new(base_count),
                        )));
                    }
                }
            }

            post_scan_busy.store(false, Ordering::Release);
            info!(
                "Rescan post-scan phase total: {:.2}s (warmup={}, content_indexing={})",
                phase_start.elapsed().as_secs_f64(),
                do_warmup,
                do_content_indexing,
            );
        });

        true
    }

    pub fn trigger_rescan(&mut self, shared_frecency: &SharedFrecency) -> Result<(), Error> {
        if self.is_scanning.load(Ordering::Relaxed) {
            debug!("Scan already in progress, skipping trigger_rescan");
            return Ok(());
        }

        // The post-scan warmup + bigram phase holds a raw pointer into the
        // current files Vec. Replacing sync_data now would free that memory.
        // Skip — the background watcher will retry on the next event.
        if self.post_scan_busy.load(Ordering::Acquire) {
            debug!("Post-scan bigram build in progress, skipping rescan");
            return Ok(());
        }

        self.is_scanning.store(true, Ordering::Relaxed);
        self.scanned_files_count.store(0, Ordering::Relaxed);

        let walk_result = walk_filesystem(
            &self.base_path,
            &self.scanned_files_count,
            shared_frecency,
            self.mode,
        );

        match walk_result {
            Ok(walk) => {
                info!(
                    "Filesystem rescan completed: found {} files",
                    walk.sync.files.len()
                );

                self.sync_data = walk.sync;
                self.cache_budget.reset();

                // Apply git status synchronously for rescan (typically fast).
                if let Ok(Some(git_cache)) = walk.git_handle.join() {
                    let frecency = shared_frecency.read().ok();
                    let frecency_ref = frecency.as_ref().and_then(|f| f.as_ref());
                    let mode = self.mode;
                    let bp = &self.base_path;
                    let arena = self.arena_base_ptr();

                    // Reset dir frecency before recomputation.
                    for dir in self.sync_data.dirs.iter() {
                        dir.reset_frecency();
                    }

                    let files = &mut self.sync_data.files;
                    let dirs = &self.sync_data.dirs;
                    BACKGROUND_THREAD_POOL.install(|| {
                        files.par_iter_mut().for_each(|file| {
                            file.git_status =
                                git_cache.lookup_status(&file.absolute_path(arena, bp));
                            if let Some(frecency) = frecency_ref {
                                let _ = file.update_frecency_scores(frecency, arena, bp, mode);
                            }
                            let score = file.access_frecency_score as i32;
                            if score > 0 {
                                let dir_idx = file.parent_dir_index() as usize;
                                if let Some(dir) = dirs.get(dir_idx) {
                                    dir.update_frecency_if_larger(score);
                                }
                            }
                        });
                    });
                }

                // Warmup is deferred to the post-rescan bigram rebuild thread
                // (spawned by trigger_full_rescan) which does warmup + bigram
                // in one pass, matching the initial scan's post-scan phase.
            }
            Err(error) => error!(?error, "Failed to scan file system"),
        }

        self.is_scanning.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Quick way to check if scan is going without acquiring a lock for [Self::get_scan_progress]
    pub fn is_scan_active(&self) -> bool {
        self.is_scanning.load(Ordering::Relaxed)
    }

    /// Return a clone of the scanning flag so callers can poll it without
    /// holding a lock on the picker.
    pub fn scan_signal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.is_scanning)
    }

    /// Return a clone of the watcher-ready flag so callers can poll it without
    /// holding a lock on the picker.
    pub fn watcher_signal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.watcher_ready)
    }
}

/// A point-in-time snapshot of the file-scanning progress.
///
/// Returned by [`FilePicker::get_scan_progress`]. Useful for displaying
/// a progress indicator while the initial scan is running.
#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub scanned_files_count: usize,
    pub is_scanning: bool,
    pub is_watcher_ready: bool,
    pub is_warmup_complete: bool,
}

#[allow(clippy::too_many_arguments)]
fn spawn_scan_and_watcher(
    base_path: PathBuf,
    scan_signal: Arc<AtomicBool>,
    watcher_ready: Arc<AtomicBool>,
    synced_files_count: Arc<AtomicUsize>,
    enable_mmap_cache: bool,
    enable_content_indexing: bool,
    watch: bool,
    mode: FFFMode,
    shared_picker: SharedPicker,
    shared_frecency: SharedFrecency,
    cancelled: Arc<AtomicBool>,
    post_scan_busy: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        // scan_signal is already `true` (set by the caller before spawning)
        // so waiters see "scanning" even before this thread is scheduled.
        info!("Starting initial file scan");

        let git_workdir;

        match walk_filesystem(&base_path, &synced_files_count, &shared_frecency, mode) {
            Ok(walk) => {
                if cancelled.load(Ordering::Acquire) {
                    info!("Walk completed but picker was replaced, discarding results");
                    scan_signal.store(false, Ordering::Relaxed);
                    return;
                }

                info!(
                    "Initial filesystem walk completed: found {} files",
                    walk.sync.files.len()
                );

                git_workdir = walk.sync.git_workdir.clone();
                let git_handle = walk.git_handle;

                // Write files immediately — they are now searchable even
                // before git status or warmup completes.
                let write_result = shared_picker.write().ok().map(|mut guard| {
                    if let Some(ref mut picker) = *guard {
                        picker.sync_data = walk.sync;
                        picker.cache_budget.reset();
                    }
                });

                if write_result.is_none() {
                    error!("Failed to write scan results into picker");
                }

                // Signal scan complete — files are searchable.
                scan_signal.store(false, Ordering::Relaxed);
                info!("Files indexed and searchable");

                if !cancelled.load(Ordering::Acquire) {
                    apply_git_status_and_frecency(
                        &shared_picker,
                        &shared_frecency,
                        git_handle,
                        mode,
                    );
                }
            }
            Err(e) => {
                error!("Initial scan failed: {:?}", e);
                scan_signal.store(false, Ordering::Relaxed);
                watcher_ready.store(true, Ordering::Release);
                return;
            }
        }

        if watch && !cancelled.load(Ordering::Acquire) {
            let watch_dirs = shared_picker
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().map(|picker| picker.extract_watch_dirs()))
                .unwrap_or_default();

            match BackgroundWatcher::new(
                base_path.clone(),
                git_workdir,
                shared_picker.clone(),
                shared_frecency.clone(),
                mode,
                watch_dirs,
            ) {
                Ok(watcher) => {
                    info!("Background file watcher initialized successfully");

                    if cancelled.load(Ordering::Acquire) {
                        info!("Picker was replaced, dropping orphaned watcher");
                        drop(watcher);
                        watcher_ready.store(true, Ordering::Release);
                        return;
                    }

                    let write_result = shared_picker.write().ok().map(|mut guard| {
                        if let Some(ref mut picker) = *guard {
                            picker.background_watcher = Some(watcher);
                        }
                    });

                    if write_result.is_none() {
                        error!("Failed to store background watcher in picker");
                    }
                }
                Err(e) => {
                    error!("Failed to initialize background file watcher: {:?}", e);
                }
            }
        }

        watcher_ready.store(true, Ordering::Release);

        let need_post_scan =
            (enable_mmap_cache || enable_content_indexing) && !cancelled.load(Ordering::Acquire);

        if need_post_scan {
            post_scan_busy.store(true, Ordering::Release);
            let phase_start = std::time::Instant::now();

            // Scale cache limits based on repo size (skip if caller provided an explicit budget).
            if let Ok(mut guard) = shared_picker.write()
                && let Some(ref mut picker) = *guard
                && !picker.has_explicit_cache_budget
            {
                let file_count = picker.sync_data.files().len();
                picker.cache_budget = Arc::new(ContentCacheBudget::new_for_repo(file_count));
                info!(
                    "Cache budget configured for {} files: max_files={}, max_bytes={}",
                    file_count, picker.cache_budget.max_files, picker.cache_budget.max_bytes,
                );
            }

            // SAFETY: The file index Vec is not resized between the initial scan
            // completing and the warmup + bigram phase finishing because
            // `post_scan_busy` prevents concurrent rescans from replacing
            // sync_data while we hold the raw pointer.
            let files_snapshot: Option<(&[FileItem], usize, Arc<ContentCacheBudget>, ArenaPtr)> =
                if !cancelled.load(Ordering::Acquire) {
                    let guard = shared_picker.read().ok();
                    guard.and_then(|guard| {
                        guard.as_ref().map(|picker| {
                            let files = picker.sync_data.files();
                            let ptr = files.as_ptr();
                            let len = files.len();
                            let base_count = picker.sync_data.base_count;
                            let budget = Arc::clone(&picker.cache_budget);
                            let arena = picker.arena_base_ptr();
                            // SAFETY: post_scan_busy flag blocks trigger_rescan and
                            // background watcher rescans from replacing sync_data,
                            // so the Vec backing this slice stays alive.
                            let static_files: &[FileItem] =
                                unsafe { std::slice::from_raw_parts(ptr, len) };
                            (static_files, base_count, budget, arena)
                        })
                    })
                } else {
                    None
                };

            // both of this is using a custom soft lock not guaranteed by compiler
            // this is required to keep the picker functioning if someone opened a really crazy
            // e.g  10m files directory but potentially unsafe
            if let Some((files, base_count, budget, arena)) = files_snapshot {
                if enable_mmap_cache && !cancelled.load(Ordering::Acquire) {
                    let warmup_start = std::time::Instant::now();
                    warmup_mmaps(files, &budget, &base_path, arena);
                    info!(
                        "Warmup completed in {:.2}s (cached {} files, {} bytes)",
                        warmup_start.elapsed().as_secs_f64(),
                        budget.cached_count.load(Ordering::Relaxed),
                        budget.cached_bytes.load(Ordering::Relaxed),
                    );
                }

                if enable_content_indexing && !cancelled.load(Ordering::Acquire) {
                    // Index ONLY base files. Any overflow files present in
                    // the snapshot (from watcher events that landed before
                    // this snapshot was taken) are intentionally excluded:
                    // grep handles them via the unconditional overflow-
                    // append loop, and the filter's `file_count` must match
                    // the overlay's `base_file_count` so the candidate
                    // bitset can't carry bits for overflow-range indices.
                    let base_files = &files[..base_count.min(files.len())];
                    let (index, content_binary) =
                        build_bigram_index(base_files, &budget, &base_path, arena);

                    if let Ok(mut guard) = shared_picker.write()
                        && let Some(ref mut picker) = *guard
                    {
                        for &idx in &content_binary {
                            if let Some(file) = picker.sync_data.get_file_mut(idx) {
                                file.set_binary(true);
                            }
                        }

                        picker.sync_data.bigram_index = Some(Arc::new(index));
                        picker.sync_data.bigram_overlay = Some(Arc::new(parking_lot::RwLock::new(
                            BigramOverlay::new(base_count),
                        )));
                    }
                }
            }

            post_scan_busy.store(false, Ordering::Release);

            info!(
                "Post-scan phase total: {:.2}s (warmup={}, content_indexing={})",
                phase_start.elapsed().as_secs_f64(),
                enable_mmap_cache,
                enable_content_indexing,
            );
        }

        // the debouncer keeps running in its own thread
    });
}

/// Pre-populate mmap caches for the most valuable files so the first grep
/// search doesn't pay the mmap creation + page fault cost.
///
/// All files are collected once, then an O(n) `select_nth_unstable_by`
/// partitions the top [`MAX_CACHED_CONTENT_FILES`] highest-frecency eligible
/// files to the front (binary / empty files are pushed to the end by the
/// comparator). The selected prefix is warmed in parallel via rayon.
///
/// Files beyond the budget are still available via temporary mmaps on first
/// grep access, so correctness is unaffected.
#[tracing::instrument(skip(files), name = "warmup_mmaps", level = Level::DEBUG)]
pub(crate) fn warmup_mmaps(
    files: &[FileItem],
    budget: &ContentCacheBudget,
    base_path: &Path,
    arena: ArenaPtr,
) {
    let max_files = budget.max_files;
    let max_bytes = budget.max_bytes;
    let max_file_size = budget.max_file_size;

    // Single collect — no pre-filter. The comparator in select_nth pushes
    // ineligible files (binary, empty) to the tail automatically.
    let mut all: Vec<&FileItem> = files.iter().collect();

    // O(n) partial sort: top max_files eligible-by-frecency files land in
    // all[..max_files]. Ineligible files compare as "lowest priority" so
    // they naturally sink past the partition boundary.
    if all.len() > max_files {
        all.select_nth_unstable_by(max_files, |a, b| {
            let a_ok = !a.is_binary() && a.size > 0;
            let b_ok = !b.is_binary() && b.size > 0;
            match (a_ok, b_ok) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                (false, false) => std::cmp::Ordering::Equal,
                (true, true) => b.total_frecency_score().cmp(&a.total_frecency_score()),
            }
        });
    }

    let to_warm = &all[..all.len().min(max_files)];

    let warmed_bytes = AtomicU64::new(0);
    let budget_exhausted = AtomicBool::new(false);

    BACKGROUND_THREAD_POOL.install(|| {
        to_warm.par_iter().for_each(|file| {
            if budget_exhausted.load(Ordering::Relaxed) {
                return;
            }

            if file.is_binary() || file.size == 0 || file.size > max_file_size {
                return;
            }

            // Byte budget.
            let prev_bytes = warmed_bytes.fetch_add(file.size, Ordering::Relaxed);
            if prev_bytes + file.size > max_bytes {
                budget_exhausted.store(true, Ordering::Relaxed);
                return;
            }

            if let Some(content) = file.get_content(arena, base_path, budget) {
                let _ = std::hint::black_box(content.first());
            }
        });
    });
}

/// Max bytes of file content scanned for bigram indexing. After this many
/// bytes the ~4900 possible printable-ASCII bigrams are effectively saturated,
/// so reading further adds no new information to the index.
pub const BIGRAM_CONTENT_CAP: usize = 64 * 1024;

#[tracing::instrument(skip_all, name = "Building Bigram Index", level = Level::DEBUG)]
pub(crate) fn build_bigram_index(
    files: &[FileItem],
    budget: &ContentCacheBudget,
    base_path: &Path,
    arena: ArenaPtr,
) -> (BigramFilter, Vec<usize>) {
    let start = std::time::Instant::now();
    info!("Building bigram index for {} files...", files.len());
    let builder = BigramIndexBuilder::new(files.len());
    let skip_builder = BigramIndexBuilder::new(files.len());
    let max_file_size = budget.max_file_size;

    // Collect indices of files that passed the extension heuristic but are
    // actually binary (contain NUL bytes). These are marked `is_binary = true`
    // on the real file list after the build, so grep never has to re-check.
    let content_binary: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

    BACKGROUND_THREAD_POOL.install(|| {
        files.par_iter().enumerate().for_each(|(i, file)| {
            if file.is_binary() || file.size == 0 || file.size > max_file_size {
                return;
            }
            // Use cached content if available (no extra memory).
            // For uncached files, read from disk — heap memory is freed on drop.
            let data: Option<&[u8]>;
            let owned;
            if let Some(cached) = file.get_content(arena, base_path, budget) {
                if detect_binary_content(cached) {
                    content_binary.lock().unwrap().push(i);
                    return;
                }
                data = Some(cached);
                owned = None;
            } else if let Ok(read_data) = std::fs::read(file.absolute_path(arena, base_path)) {
                if detect_binary_content(&read_data) {
                    content_binary.lock().unwrap().push(i);
                    return;
                }
                data = None;
                owned = Some(read_data);
            } else {
                return;
            }

            let content = data.unwrap_or_else(|| owned.as_ref().unwrap());
            let capped = &content[..content.len().min(BIGRAM_CONTENT_CAP)];
            builder.add_file_content(&skip_builder, i, capped);
        });
    });

    let cols = builder.columns_used();
    let mut index = builder.compress(None);

    // Skip bigrams are supplementary — the consecutive index does the heavy
    // lifting. Rare skip columns (< 12% of files) add virtually no filtering
    // on either homogeneous (kernel) or polyglot (monorepo) codebases, but
    // cost ~25-30% of total index memory. Using a higher sparse cutoff for
    // the skip index drops these dead-weight columns with negligible loss.
    let skip_index = skip_builder.compress(Some(12));
    index.set_skip_index(skip_index);

    // The builders' flat buffers were freed by compress() above (single
    // deallocation each). Hint the allocator to return pages from other
    // per-thread allocations (file reads, sort buffers) during the build.
    hint_allocator_collect();

    info!(
        "Bigram index built in {:.2}s — {} dense columns for {} files",
        start.elapsed().as_secs_f64(),
        cols,
        files.len(),
    );

    let binary_indices = content_binary.into_inner().unwrap();
    if !binary_indices.is_empty() {
        info!(
            "Bigram build detected {} content-binary files (not caught by extension)",
            binary_indices.len(),
        );
    }

    (index, binary_indices)
}

/// Result of the fast walk phase — files are searchable immediately,
/// git status arrives later via the join handle.
struct WalkResult {
    sync: FileSync,
    git_handle: std::thread::JoinHandle<Option<GitStatusCache>>,
}

/// Returns files immediately (searchable) and a handle to the in-progress
/// git status computation. This avoids blocking on `git status` which can
/// take 10+ seconds on very large repos (e.g. chromium).
fn walk_filesystem(
    base_path: &Path,
    synced_files_count: &Arc<AtomicUsize>,
    shared_frecency: &SharedFrecency,
    mode: FFFMode,
) -> Result<WalkResult, Error> {
    use ignore::WalkBuilder;

    let scan_start = std::time::Instant::now();
    info!("SCAN: Starting filesystem walk and git status (async)");

    // Discover git root (fast — just walks up looking for .git/)
    let git_workdir = Repository::discover(base_path)
        .ok()
        .and_then(|repo| repo.workdir().map(Path::to_path_buf));

    if let Some(ref git_dir) = git_workdir {
        debug!("Git repository found at: {}", git_dir.display());
    } else {
        debug!("No git repository found for path: {}", base_path.display());
    }

    // Spawn git status on a detached thread — we won't wait for it here.
    let git_workdir_for_status = git_workdir.clone();
    let git_handle = std::thread::spawn(move || {
        GitStatusCache::read_git_status(
            git_workdir_for_status.as_deref(),
            StatusOptions::new()
                .include_untracked(true)
                .recurse_untracked_dirs(true)
                .exclude_submodules(true),
        )
    });

    // Walk files (the fast part, typically 2-3s even on huge repos).
    let is_git_repo = git_workdir.is_some();
    let bg_threads = BACKGROUND_THREAD_POOL.current_num_threads();
    let mut walk_builder = WalkBuilder::new(base_path);
    walk_builder
        // this is a very important guard for the user opening ~/ or other root non-git dir
        .hidden(!is_git_repo)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .follow_links(false)
        .threads(bg_threads);

    if !is_git_repo && let Some(overrides) = non_git_repo_overrides(base_path) {
        walk_builder.overrides(overrides);
    }

    let walker = walk_builder.build_parallel();

    let walker_start = std::time::Instant::now();
    debug!("SCAN: Starting file walker");

    // Walk: collect (FileItem, rel_path) pairs. Keep the walk fast —
    // no chunking, no HashMap, just Vec::push under the Mutex.
    let pairs = parking_lot::Mutex::new(Vec::<(FileItem, String)>::new());

    walker.run(|| {
        let pairs = &pairs;
        let counter = Arc::clone(synced_files_count);
        let base_path = base_path.to_path_buf();

        Box::new(move |result| {
            let Ok(entry) = result else {
                return ignore::WalkState::Continue;
            };

            if entry.file_type().is_some_and(|ft| ft.is_file()) {
                let path = entry.path();

                if is_git_file(path) {
                    return ignore::WalkState::Continue;
                }

                if !is_git_repo && is_known_binary_extension(path) {
                    return ignore::WalkState::Continue;
                }

                let metadata = entry.metadata().ok();
                let (file_item, rel_path) =
                    FileItem::new_from_walk(path, &base_path, None, metadata.as_ref());

                pairs.lock().push((file_item, rel_path));
                counter.fetch_add(1, Ordering::Relaxed);
            }
            ignore::WalkState::Continue
        })
    });

    let mut pairs = pairs.into_inner();

    info!(
        "SCAN: File walking completed in {:?} for {} files",
        walker_start.elapsed(),
        pairs.len(),
    );

    // Sort by full relative path. This groups files by directory naturally,
    // so dir extraction becomes a simple linear scan — no HashMap.
    BACKGROUND_THREAD_POOL.install(|| {
        pairs.par_sort_unstable_by(|(_, a), (_, b)| a.cmp(b));
    });

    // Build ChunkedPathStore + extract dirs + assign parent_dir in one pass.
    // Files are sorted by relative path, so dir changes happen in order.
    // add_file_immediate returns a ChunkedString with null arena_base;
    // we fixup arena_base after the arena is frozen.
    let mut files: Vec<FileItem> = Vec::with_capacity(pairs.len());
    let mut dirs: Vec<DirItem> = Vec::new();
    let mut builder = crate::simd_path::ChunkedPathStoreBuilder::new(pairs.len());
    // Use a sentinel that can never match any real dir_part (including "")
    // so the very first file always creates its dir entry.
    let mut prev_dir: Option<String> = None;
    let mut current_dir_idx: u32 = 0;

    for (mut file, rel) in pairs {
        let fname_offset = file.path.filename_offset as usize;
        let dir_part = &rel[..fname_offset];

        if prev_dir.as_deref() != Some(dir_part) {
            let dir_cs = builder.add_dir_immediate(dir_part);
            // Compute last-segment offset: for "src/components/" -> 4 (points to "components/")
            let last_seg = if dir_part.is_empty() {
                0
            } else {
                let trimmed = dir_part.trim_end_matches(std::path::is_separator);
                trimmed
                    .rfind(std::path::is_separator)
                    .map(|i| i + 1)
                    .unwrap_or(0) as u16
            };
            dirs.push(DirItem::new(dir_cs, last_seg));
            current_dir_idx = (dirs.len() - 1) as u32;
            prev_dir = Some(dir_part.to_string());
        }

        let cs = builder.add_file_immediate(&rel, file.path.filename_offset);
        file.set_path(cs);
        file.set_parent_dir(current_dir_idx);
        files.push(file);
    }
    let chunked_paths = builder.finish();
    let arena = chunked_paths.as_arena_ptr();

    // Apply frecency scores (access-based only — git status not yet available).
    // DirItem.max_access_frecency is AtomicI32, so parallel threads write directly.
    let frecency = shared_frecency
        .read()
        .map_err(|_| Error::AcquireFrecencyLock)?;
    if let Some(frecency) = frecency.as_ref() {
        let dirs_ref = &dirs;
        BACKGROUND_THREAD_POOL.install(|| {
            files.par_iter_mut().for_each(|file| {
                let _ = file.update_frecency_scores(frecency, arena, base_path, mode);
                let score = file.access_frecency_score as i32;
                if score > 0 {
                    let dir_idx = file.parent_dir_index() as usize;
                    if let Some(dir) = dirs_ref.get(dir_idx) {
                        dir.update_frecency_if_larger(score);
                    }
                }
            });
        });
    }
    drop(frecency);

    // Re-sort by (parent_dir, filename) for binary search in find_file_index.
    BACKGROUND_THREAD_POOL.install(|| {
        files.par_sort_unstable_by(|a, b| {
            a.parent_dir_index()
                .cmp(&b.parent_dir_index())
                .then_with(|| a.file_name(arena).cmp(&b.file_name(arena)))
        });
    });

    // Ask the allocator to return freed pages to the OS.
    hint_allocator_collect();

    let file_item_size = std::mem::size_of::<FileItem>();
    let files_vec_bytes = files.len() * file_item_size;
    let dir_table_bytes = dirs.len() * std::mem::size_of::<DirItem>()
        + dirs
            .iter()
            .map(|d| d.relative_path(arena).len())
            .sum::<usize>();

    let total_time = scan_start.elapsed();
    info!(
        "SCAN: Walk completed in {:?} ({} files, {} dirs, \
         chunked_store={:.2}MB, files_vec={:.2}MB, dirs={:.2}MB, FileItem={}B)",
        total_time,
        files.len(),
        dirs.len(),
        chunked_paths.heap_bytes() as f64 / 1_048_576.0,
        files_vec_bytes as f64 / 1_048_576.0,
        dir_table_bytes as f64 / 1_048_576.0,
        file_item_size,
    );

    let base_count = files.len();

    Ok(WalkResult {
        sync: FileSync {
            files,
            base_count,
            dirs,
            overflow_builder: None,
            git_workdir,
            bigram_index: None,
            bigram_overlay: None,
            chunked_paths: Some(chunked_paths),
        },
        git_handle,
    })
}

fn apply_git_status_and_frecency(
    shared_picker: &SharedPicker,
    shared_frecency: &SharedFrecency,
    git_handle: std::thread::JoinHandle<Option<GitStatusCache>>,
    mode: FFFMode,
) {
    let join_start = std::time::Instant::now();
    let git_cache = match git_handle.join() {
        Ok(cache) => cache,
        Err(_) => {
            error!("Git status thread panicked");
            return;
        }
    };
    info!("SCAN: Git status ready in {:?}", join_start.elapsed());

    let Some(git_cache) = git_cache else { return };

    if let Ok(mut guard) = shared_picker.write()
        && let Some(ref mut picker) = *guard
    {
        let frecency = shared_frecency.read().ok();
        let frecency_ref = frecency.as_ref().and_then(|f| f.as_ref());

        // Destructure to split borrows: files (mut) and dirs (shared) are independent.
        let bp = &picker.base_path;
        let arena = picker.arena_base_ptr();

        // Reset dir frecency before recomputation.
        for dir in picker.sync_data.dirs.iter() {
            dir.reset_frecency();
        }

        let files = &mut picker.sync_data.files;
        let dirs = &picker.sync_data.dirs;

        BACKGROUND_THREAD_POOL.install(|| {
            files.par_iter_mut().for_each(|file| {
                let mut buf = [0u8; crate::simd_path::PATH_BUF_SIZE];
                let absolute_path = file.write_absolute_path(arena, bp, &mut buf);

                file.git_status = git_cache.lookup_status(absolute_path);
                if let Some(frecency) = frecency_ref {
                    let _ = file.update_frecency_scores(frecency, arena, bp, mode);
                }

                let score = file.access_frecency_score as i32;
                if score > 0 {
                    let dir_idx = file.parent_dir_index() as usize;
                    if let Some(dir) = dirs.get(dir_idx) {
                        dir.update_frecency_if_larger(score);
                    }
                }
            });
        });

        info!(
            "SCAN: Applied git status to {} files ({} dirty)",
            picker.sync_data.files.len(),
            git_cache.statuses_len(),
        );
    }
}

#[inline]
fn is_git_file(path: &Path) -> bool {
    path.to_str().is_some_and(|path| {
        if cfg!(target_family = "windows") {
            path.contains("\\.git\\")
        } else {
            path.contains("/.git/")
        }
    })
}

/// Fast extension-based binary detection. Avoids opening files during scan.
/// Covers the vast majority of binary files in typical repositories.
#[inline]
fn is_known_binary_extension(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext,
        // Images
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "ico" | "webp" | "tiff" | "tif" | "avif" |
        "heic" | "psd" | "icns" | "cur" | "raw" | "cr2" | "nef" | "dng" |
        // Video/Audio
        "mp4" | "avi" | "mov" | "wmv" | "mkv" | "mp3" | "wav" | "flac" | "ogg" | "m4a" |
        "aac" | "webm" | "flv" | "mpg" | "mpeg" | "wma" | "opus" |
        // Compressed/Archives
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "zst" | "lz4" | "lzma" |
        "cab" | "cpio" |
        // Packages/Installers
        "deb" | "rpm" | "apk" | "dmg" | "msi" | "iso" | "nupkg" | "whl" | "egg" |
        "snap" | "appimage" | "flatpak" |
        // Executables/Libraries
        "exe" | "dll" | "so" | "dylib" | "o" | "a" | "lib" | "bin" | "elf" |
        // Documents
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" |
        // Databases
        "db" | "sqlite" | "sqlite3" | "mdb" |
        // Fonts
        "ttf" | "otf" | "woff" | "woff2" | "eot" |
        // Compiled/Runtime
        "class" | "pyc" | "pyo" | "wasm" | "dex" | "jar" | "war" |
        // ML/Data Science
        "npy" | "npz" | "pkl" | "pickle" | "h5" | "hdf5" | "pt" | "pth" | "onnx" |
        "safetensors" | "tfrecord" |
        // 3D/Game
        "glb" | "fbx" | "blend" |
        // Data/serialized
        "parquet" | "arrow" | "pb" |
        // IDE/OS metadata
        "DS_Store" | "suo"
    )
}

/// Detect binary content by checking for NUL bytes in the first 512 bytes.
/// Called lazily when file content is first loaded, not during initial scan.
#[inline]
pub(crate) fn detect_binary_content(content: &[u8]) -> bool {
    let check_len = content.len().min(512);
    content[..check_len].contains(&0)
}

/// Ask the global allocator to return freed pages to the OS.
/// Enabled via the `mimalloc-collect` feature (set by fff-nvim).
/// No-op when the feature is off (tests, system allocator).
fn hint_allocator_collect() {
    #[cfg(feature = "mimalloc-collect")]
    {
        // Collect BACKGROUND_THREAD_POOL workers — that's where the bigram
        // builder allocated memory. `rayon::broadcast` would target the global
        // pool, which is the wrong set of threads.
        BACKGROUND_THREAD_POOL.broadcast(|_| unsafe { libmimalloc_sys::mi_collect(true) });

        // Main thread too.
        unsafe { libmimalloc_sys::mi_collect(true) };
    }
}
