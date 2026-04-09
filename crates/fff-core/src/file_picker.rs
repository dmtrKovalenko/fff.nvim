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
//!   fuzzy_search()   <── static, borrows &[FileItem]
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

use crate::background_watcher::BackgroundWatcher;
use crate::bigram_filter::{BigramFilter, BigramIndexBuilder, BigramOverlay};
use crate::error::Error;
use crate::frecency::FrecencyTracker;
use crate::git::GitStatusCache;
use crate::grep::{GrepResult, GrepSearchOptions, grep_search};
use crate::ignore::non_git_repo_overrides;
use crate::query_tracker::QueryTracker;
use crate::score::match_and_score_files;
use crate::shared::{SharedFrecency, SharedPicker};
use crate::types::{ContentCacheBudget, FileItem, PaginationArgs, ScoringContext, SearchResult};
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
/// Passed to [`FilePicker::fuzzy_search`] to control threading, pagination,
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
    /// All files: `files[..base_count]` are sorted by path (base index, used
    /// for binary search and bigram);
    ///
    /// `files[base_count..]` are overflow files added since the last full reindex.
    /// Deletions in the base use tombstones (`is_deleted = true`) to keep bigram indices stable.
    files: Vec<FileItem>,
    /// Number of base files (the sorted prefix used for binary search / bigram).
    base_count: usize,
    /// Compressed bigram inverted index built during the post-scan phase.
    /// Lives here so that replacing `FileSync` on rescan automatically drops
    /// the stale index (bigram file indices are positions in `files`).
    bigram_index: Option<Arc<BigramFilter>>,
    /// Overlay tracking file mutations since the bigram index was built.
    bigram_overlay: Option<Arc<parking_lot::RwLock<BigramOverlay>>>,
}

impl FileSync {
    fn new() -> Self {
        Self {
            files: Vec::new(),
            base_count: 0,
            git_workdir: None,
            bigram_index: None,
            bigram_overlay: None,
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

    #[allow(dead_code)]
    fn get_file(&self, index: usize) -> Option<&FileItem> {
        self.files.get(index)
    }

    /// Get mutable file at index (works for both base and overflow)
    #[inline]
    fn get_file_mut(&mut self, index: usize) -> Option<&mut FileItem> {
        self.files.get_mut(index)
    }

    /// Find file index by path using binary search on the sorted base portion.
    #[inline]
    fn find_file_index(&self, path: &Path) -> Result<usize, usize> {
        self.files[..self.base_count].binary_search_by(|f| f.as_path().cmp(path))
    }

    /// Find a file in the overflow portion by path (linear scan).
    /// Returns the absolute index into `files`.
    ///
    /// the overflowed items are not ordered so we can not use binary search
    fn find_overflow_index(&self, path: &Path) -> Option<usize> {
        self.files[self.base_count..]
            .iter()
            .position(|f| f.as_path() == path)
            .map(|pos| self.base_count + pos)
    }

    /// Get file count
    #[inline]
    #[allow(dead_code)]
    fn len(&self) -> usize {
        self.files.len()
    }

    /// Insert a file at position. Simple - no HashMap to maintain!
    fn insert_file(&mut self, position: usize, file: FileItem) {
        self.files.insert(position, file);
    }

    /// Remove file at index. Simple - no HashMap to maintain!
    #[allow(dead_code)]
    fn remove_file(&mut self, index: usize) {
        if index < self.files.len() {
            self.files.remove(index);
        }
    }

    /// Remove files matching predicate from both base and overflow.
    /// Returns number of files removed. Adjusts `base_count` accordingly.
    fn retain_files<F>(&mut self, mut predicate: F) -> usize
    where
        F: FnMut(&FileItem) -> bool,
    {
        let initial_len = self.files.len();
        // Count how many base files survive.
        let base_retained = self.files[..self.base_count]
            .iter()
            .filter(|f| predicate(f))
            .count();
        self.files.retain(predicate);
        self.base_count = base_retained;
        initial_len - self.files.len()
    }

    /// Insert a file in sorted order (by path).
    /// Returns true if inserted, false if file already exists.
    fn insert_file_sorted(&mut self, file: FileItem) -> bool {
        match self.find_file_index(file.as_path()) {
            Ok(_) => false, // File already exists
            Err(position) => {
                self.insert_file(position, file);
                true
            }
        }
    }
}

impl FileItem {
    pub fn new(path: PathBuf, base_path: &Path, git_status: Option<Status>) -> Self {
        let metadata = std::fs::metadata(&path).ok();
        Self::new_with_metadata(path, base_path, git_status, metadata.as_ref())
    }

    /// Create a FileItem using pre-fetched metadata to avoid a redundant stat syscall.
    pub fn new_with_metadata(
        path: PathBuf,
        base_path: &Path,
        git_status: Option<Status>,
        metadata: Option<&std::fs::Metadata>,
    ) -> Self {
        let relative_path = pathdiff::diff_paths(&path, base_path)
            .unwrap_or_else(|| path.clone())
            .to_string_lossy()
            .into_owned();

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

        // Fast extension-based binary detection avoids opening every file during scan.
        // Files not caught here are detected when content is first loaded.
        let is_binary = is_known_binary_extension(&path);

        let path_string = path.to_string_lossy().into_owned();
        let relative_start = (path_string.len() - relative_path.len()) as u16;
        let filename_start = path_string
            .rfind(std::path::MAIN_SEPARATOR)
            .map(|i| i + 1)
            .unwrap_or(relative_start as usize) as u16;

        Self::new_raw(
            path_string,
            relative_start,
            filename_start,
            size,
            modified,
            git_status,
            is_binary,
        )
    }

    pub fn update_frecency_scores(
        &mut self,
        tracker: &FrecencyTracker,
        mode: FFFMode,
    ) -> Result<(), Error> {
        self.access_frecency_score = tracker.get_access_score(self.as_path(), mode) as i16;
        self.modification_frecency_score =
            tracker.get_modification_score(self.modified, self.git_status, mode) as i16;

        Ok(())
    }
}

/// Options for creating a [`FilePicker`].
pub struct FilePickerOptions {
    pub base_path: String,
    pub warmup_mmap_cache: bool,
    pub mode: FFFMode,
    /// Explicit cache budget. When `None`, the budget is auto-computed from
    /// the repo size after the initial scan completes.
    pub cache_budget: Option<ContentCacheBudget>,
    /// When `false`, `new_with_shared_state` skips the background file watcher.
    /// Files are still scanned, warmed up, and bigram-indexed.
    pub watch: bool,
}

impl Default for FilePickerOptions {
    fn default() -> Self {
        Self {
            base_path: ".".into(),
            warmup_mmap_cache: false,
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
    warmup_mmap_cache: bool,
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

impl FilePicker {
    pub fn base_path(&self) -> &Path {
        &self.base_path
    }

    pub fn need_warmup_mmap_cache(&self) -> bool {
        self.warmup_mmap_cache
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

    /// Extracts all unique ancestor directories from the indexed file list.
    pub fn extract_watch_dirs(&self) -> Vec<PathBuf> {
        let files = self.sync_data.files();
        let base = self.base_path.as_path();
        let mut dirs = Vec::with_capacity(files.len() / 4);
        let mut current = self.base_path.clone();

        for file in files {
            let Some(parent) = file.as_path().parent() else {
                continue;
            };
            if parent == current.as_path() {
                continue;
            }

            // Pop up to the common ancestor of current and parent.
            while current.as_path() != base && !parent.starts_with(&current) {
                current.pop();
            }

            // Push down to parent, emitting each new directory level.
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
            warmup_mmap_cache: options.warmup_mmap_cache,
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
            "Spawning background threads: base_path={}, warmup={}, mode={:?}",
            picker.base_path.display(),
            picker.warmup_mmap_cache,
            picker.mode,
        );

        let warmup = picker.warmup_mmap_cache;
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
            for file in self.sync_data.files.iter_mut() {
                file.git_status = git_cache.lookup_status(file.as_path());
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
    pub fn fuzzy_search<'a, 'q>(
        files: &'a [FileItem],
        query: &'q FFFQuery<'q>,
        query_tracker: Option<&QueryTracker>,
        options: FuzzySearchOptions<'q>,
    ) -> SearchResult<'a> {
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
        let (items, scores, total_matched) = match_and_score_files(files, &context);

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

    /// Perform a live grep search across indexed files with a pre-parsed query.
    pub fn grep(&self, query: &FFFQuery<'_>, options: &GrepSearchOptions) -> GrepResult<'_> {
        let overlay_guard = self.sync_data.bigram_overlay.as_ref().map(|o| o.read());
        grep_search(
            self.get_files(),
            query,
            options,
            self.cache_budget(),
            self.sync_data.bigram_index.as_deref(),
            overlay_guard.as_deref(),
            Some(&self.cancelled),
        )
    }

    /// Like [`grep`](Self::grep) but ignores the bigram overlay.
    /// Useful for testing that the overlay is actually contributing results.
    pub fn grep_without_overlay(
        &self,
        query: &FFFQuery<'_>,
        options: &GrepSearchOptions,
    ) -> GrepResult<'_> {
        grep_search(
            self.get_files(),
            query,
            options,
            self.cache_budget(),
            self.sync_data.bigram_index.as_deref(),
            None,
            Some(&self.cancelled),
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
        let frecency = shared_frecency.read()?;
        status_cache
            .into_iter()
            .try_for_each(|(path, status)| -> Result<(), Error> {
                if let Some(file) = self.get_mut_file_by_path(&path) {
                    file.git_status = Some(status);
                    if let Some(ref f) = *frecency {
                        file.update_frecency_scores(f, mode)?;
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
        let index = self
            .sync_data
            .find_file_index(path)
            .ok()
            .or_else(|| self.sync_data.find_overflow_index(path));
        if let Some(index) = index
            && let Some(file) = self.sync_data.get_file_mut(index)
        {
            file.update_frecency_scores(frecency_tracker, self.mode)?;
        }

        Ok(())
    }

    pub fn get_file_by_path(&self, path: impl AsRef<Path>) -> Option<&FileItem> {
        self.sync_data
            .find_file_index(path.as_ref())
            .ok()
            .and_then(|index| self.sync_data.files().get(index))
    }

    pub fn get_mut_file_by_path(&mut self, path: impl AsRef<Path>) -> Option<&mut FileItem> {
        let path = path.as_ref();
        // Check sorted base first (O(log n)), then overflow tail (O(k)).
        let index = self
            .sync_data
            .find_file_index(path)
            .ok()
            .or_else(|| self.sync_data.find_overflow_index(path));
        index.and_then(|i| self.sync_data.get_file_mut(i))
    }

    /// Add a file to the picker's files in sorted order (used by background watcher)
    pub fn add_file_sorted(&mut self, file: FileItem) -> Option<&FileItem> {
        let path = PathBuf::from(file.path_str());

        if self.sync_data.insert_file_sorted(file) {
            // File was inserted, look it up
            self.sync_data
                .find_file_index(&path)
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
                .find_file_index(&path)
                .ok()
                .and_then(|idx| self.sync_data.get_file_mut(idx))
                .map(|file_mut| &*file_mut) // Convert &mut to &
        }
    }

    #[tracing::instrument(skip(self), name = "timing_update", level = Level::DEBUG)]
    pub fn on_create_or_modify(&mut self, path: impl AsRef<Path> + Debug) -> Option<&FileItem> {
        let path = path.as_ref();

        // Clone the overlay Arc upfront so we can access it independently of
        // the mutable borrow on sync_data.files (just a refcount bump).
        let overlay = self.sync_data.bigram_overlay.clone();

        // Check if this is a tombstoned base file being re-created.
        if let Ok(pos) = self.sync_data.find_file_index(path) {
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
        if let Some(abs_pos) = self.sync_data.find_overflow_index(path) {
            let file = &mut self.sync_data.files[abs_pos];
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
            return Some(&self.sync_data.files[abs_pos]);
        }

        // New file — append to overflow tail (preserves base indices for bigram).
        debug!(
            "on_create_or_modify: file NEW, appending to overflow (base: {}, overflow: {})",
            self.sync_data.base_count,
            self.sync_data.overflow_files().len(),
        );

        let file_item = FileItem::new(path.to_path_buf(), &self.base_path, None);
        self.sync_data.files.push(file_item);

        self.sync_data.files.last()
    }

    /// Tombstone a file instead of removing it, keeping base indices stable.
    pub fn remove_file_by_path(&mut self, path: impl AsRef<Path>) -> bool {
        let path = path.as_ref();
        match self.sync_data.find_file_index(path) {
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
                if let Some(abs_pos) = self.sync_data.find_overflow_index(path) {
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
        // Use the safe retain_files method which maintains both indices
        self.sync_data
            .retain_files(|file| !file.as_path().starts_with(dir_path))
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
                    BACKGROUND_THREAD_POOL.install(|| {
                        self.sync_data.files.par_iter_mut().for_each(|file| {
                            file.git_status = git_cache.lookup_status(file.as_path());
                            if let Some(frecency) = frecency_ref {
                                let _ = file.update_frecency_scores(frecency, mode);
                            }
                        });
                    });
                }

                if self.warmup_mmap_cache {
                    let files = self.sync_data.files().to_vec();
                    let budget = Arc::clone(&self.cache_budget);
                    std::thread::spawn(move || {
                        warmup_mmaps(&files, &budget);
                    });
                }
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
#[allow(unused)]
#[derive(Debug, Clone)]
pub struct ScanProgress {
    /// Number of files indexed so far.
    pub scanned_files_count: usize,
    /// `true` while the background scan thread is still running.
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
    warmup_mmap_cache: bool,
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

                // Apply git status (may still be running — this waits for it).
                if !cancelled.load(Ordering::Acquire) {
                    apply_git_status(&shared_picker, &shared_frecency, git_handle, mode);
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
                base_path,
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

        if warmup_mmap_cache && !cancelled.load(Ordering::Acquire) {
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
            let files_snapshot: Option<(&[FileItem], Arc<ContentCacheBudget>)> =
                if !cancelled.load(Ordering::Acquire) {
                    let guard = shared_picker.read().ok();
                    guard.and_then(|guard| {
                        guard.as_ref().map(|picker| {
                            let files = picker.sync_data.files();
                            let ptr = files.as_ptr();
                            let len = files.len();
                            let budget = Arc::clone(&picker.cache_budget);
                            // SAFETY: post_scan_busy flag blocks trigger_rescan and
                            // background watcher rescans from replacing sync_data,
                            // so the Vec backing this slice stays alive.
                            let static_files: &[FileItem] =
                                unsafe { std::slice::from_raw_parts(ptr, len) };
                            (static_files, budget)
                        })
                    })
                } else {
                    None
                };

            if let Some((files, budget)) = files_snapshot {
                // Warmup: populate mmap caches for top-frecency files.
                if !cancelled.load(Ordering::Acquire) {
                    let warmup_start = std::time::Instant::now();
                    warmup_mmaps(files, &budget);
                    info!(
                        "Warmup completed in {:.2}s (cached {} files, {} bytes)",
                        warmup_start.elapsed().as_secs_f64(),
                        budget.cached_count.load(Ordering::Relaxed),
                        budget.cached_bytes.load(Ordering::Relaxed),
                    );
                }

                // Build bigram index — entirely lock-free.
                if !cancelled.load(Ordering::Acquire) {
                    let bigram_start = std::time::Instant::now();
                    info!("Starting bigram index build for {} files...", files.len());
                    let (index, content_binary) = build_bigram_index(files, &budget);
                    info!(
                        "Bigram index ready in {:.2}s",
                        bigram_start.elapsed().as_secs_f64(),
                    );

                    if let Ok(mut guard) = shared_picker.write()
                        && let Some(ref mut picker) = *guard
                    {
                        for &idx in &content_binary {
                            if let Some(file) = picker.sync_data.get_file_mut(idx) {
                                file.set_binary(true);
                            }
                        }

                        let base_count = picker.sync_data.base_count;
                        picker.sync_data.bigram_index = Some(Arc::new(index));
                        picker.sync_data.bigram_overlay = Some(Arc::new(parking_lot::RwLock::new(
                            BigramOverlay::new(base_count),
                        )));
                    }
                }
            }

            post_scan_busy.store(false, Ordering::Release);

            info!(
                "Post-scan warmup + bigram total: {:.2}s",
                phase_start.elapsed().as_secs_f64(),
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
pub fn warmup_mmaps(files: &[FileItem], budget: &ContentCacheBudget) {
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

            if let Some(content) = file.get_content(budget) {
                let _ = std::hint::black_box(content.first());
            }
        });
    });
}

/// Max bytes of file content scanned for bigram indexing. After this many
/// bytes the ~4900 possible printable-ASCII bigrams are effectively saturated,
/// so reading further adds no new information to the index.
pub const BIGRAM_CONTENT_CAP: usize = 64 * 1024;

pub fn build_bigram_index(
    files: &[FileItem],
    budget: &ContentCacheBudget,
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
            if let Some(cached) = file.get_content(budget) {
                if detect_binary_content(cached) {
                    content_binary.lock().unwrap().push(i);
                    return;
                }
                data = Some(cached);
                owned = None;
            } else if let Ok(read_data) = std::fs::read(file.as_path()) {
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

// pub for benchmarks
pub fn scan_files(base_path: &Path) -> Vec<FileItem> {
    use ignore::{WalkBuilder, WalkState};

    let git_workdir = Repository::discover(base_path)
        .ok()
        .and_then(|repo| repo.workdir().map(Path::to_path_buf));
    let is_git_repo = git_workdir.is_some();

    let mut walk_builder = WalkBuilder::new(base_path);
    walk_builder
        .hidden(!is_git_repo)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .follow_links(false);

    if !is_git_repo && let Some(overrides) = non_git_repo_overrides(base_path) {
        walk_builder.overrides(overrides);
    }

    let walker = walk_builder.build_parallel();
    let files = parking_lot::Mutex::new(Vec::new());

    walker.run(|| {
        let files = &files;
        let base_path = base_path.to_path_buf();

        Box::new(move |result| {
            let Ok(entry) = result else {
                return WalkState::Continue;
            };

            if entry.file_type().is_some_and(|ft| ft.is_file()) {
                let path = entry.path();

                if is_git_file(path) {
                    return WalkState::Continue;
                }

                if !is_git_repo && is_known_binary_extension(path) {
                    return WalkState::Continue;
                }

                let metadata = entry.metadata().ok();
                let file_item = FileItem::new_with_metadata(
                    path.to_path_buf(),
                    &base_path,
                    None,
                    metadata.as_ref(),
                );

                files.lock().push(file_item);
            }
            WalkState::Continue
        })
    });

    let mut files = files.into_inner();
    files.sort_unstable_by(|a, b| a.path_str().cmp(b.path_str()));
    files
}

/// Result of the fast walk phase — files are searchable immediately,
/// git status arrives later via the join handle.
struct WalkResult {
    sync: FileSync,
    git_handle: std::thread::JoinHandle<Option<GitStatusCache>>,
}

/// Phase 1: walk the filesystem and discover the git root.
/// Returns files immediately (searchable) and a handle to the in-progress
/// git status computation. This avoids blocking on `git status` which can
/// take 10+ seconds on very large repos (e.g. chromium).
fn walk_filesystem(
    base_path: &Path,
    synced_files_count: &Arc<AtomicUsize>,
    shared_frecency: &SharedFrecency,
    mode: FFFMode,
) -> Result<WalkResult, Error> {
    use ignore::{WalkBuilder, WalkState};

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

    let files = parking_lot::Mutex::new(Vec::new());
    walker.run(|| {
        let files = &files;
        let counter = Arc::clone(synced_files_count);
        let base_path = base_path.to_path_buf();

        Box::new(move |result| {
            let Ok(entry) = result else {
                return WalkState::Continue;
            };

            if entry.file_type().is_some_and(|ft| ft.is_file()) {
                let path = entry.path();

                if is_git_file(path) {
                    return WalkState::Continue;
                }

                // Outside git repos, skip binary files entirely — they inflate
                // the index with media, compiled artifacts, etc. that are never
                // useful in a code finder.
                if !is_git_repo && is_known_binary_extension(path) {
                    return WalkState::Continue;
                }

                let metadata = entry.metadata().ok();
                let file_item = FileItem::new_with_metadata(
                    path.to_path_buf(),
                    &base_path,
                    None,
                    metadata.as_ref(),
                );

                files.lock().push(file_item);
                counter.fetch_add(1, Ordering::Relaxed);
            }
            WalkState::Continue
        })
    });

    let mut files = files.into_inner();
    info!(
        "SCAN: File walking completed in {:?} for {} files",
        walker_start.elapsed(),
        files.len(),
    );

    // Apply frecency scores (access-based only — git status not yet available).
    let frecency = shared_frecency
        .read()
        .map_err(|_| Error::AcquireFrecencyLock)?;
    if let Some(frecency) = frecency.as_ref() {
        BACKGROUND_THREAD_POOL.install(|| {
            files.par_iter_mut().for_each(|file| {
                let _ = file.update_frecency_scores(frecency, mode);
            });
        });
    }
    drop(frecency);

    BACKGROUND_THREAD_POOL.install(|| {
        files.par_sort_unstable_by(|a, b| a.path_str().cmp(b.path_str()));
    });

    let total_time = scan_start.elapsed();
    info!("SCAN: Walk + frecency completed in {:?}", total_time);

    let base_count = files.len();
    Ok(WalkResult {
        sync: FileSync {
            files,
            base_count,
            git_workdir,
            bigram_index: None,
            bigram_overlay: None,
        },
        git_handle,
    })
}

/// Phase 2: apply git status to already-indexed files and recalculate
/// frecency scores that depend on it.
fn apply_git_status(
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

        BACKGROUND_THREAD_POOL.install(|| {
            picker.sync_data.files.par_iter_mut().for_each(|file| {
                file.git_status = git_cache.lookup_status(file.as_path());
                if let Some(frecency) = frecency_ref {
                    let _ = file.update_frecency_scores(frecency, mode);
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
