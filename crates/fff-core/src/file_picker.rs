use crate::background_watcher::BackgroundWatcher;
use crate::error::Error;
use crate::frecency::FrecencyTracker;
use crate::git::GitStatusCache;
use crate::query_tracker::QueryMatchEntry;
use crate::score::match_and_score_files;
use crate::types::{FileItem, PaginationArgs, ScoringContext, SearchResult};
use crate::{SharedFrecency, SharedPicker};
use ahash::AHashMap;
use fff_query_parser::FFFQuery;
use git2::{Repository, Status, StatusOptions};
use rayon::prelude::*;
use std::fmt::Debug;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::SystemTime;
use tracing::{Level, debug, error, info, warn};

use crate::{FILE_PICKER, FRECENCY};

/// Detect if a file is binary by checking for NUL bytes in the first 512 bytes.
/// This is the same heuristic used by git and grep — simple, fast, and sufficient.
#[inline]
fn detect_binary(path: &Path, size: u64) -> bool {
    // Empty files are not binary
    if size == 0 {
        return false;
    }

    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut reader = std::io::BufReader::with_capacity(1024, file);

    let mut buf = [0u8; 512];
    let n = reader.read(&mut buf).unwrap_or(0);
    buf[..n].contains(&0)
}

#[derive(Debug, Clone, Copy)]
pub struct FuzzySearchOptions<'a> {
    pub max_threads: usize,
    pub current_file: Option<&'a str>,
    pub project_path: Option<&'a Path>,
    pub last_same_query_match: Option<&'a QueryMatchEntry>,
    pub combo_boost_score_multiplier: i32,
    pub min_combo_count: u32,
    pub pagination: PaginationArgs,
}

#[derive(Debug, Clone)]
struct FileSync {
    pub files: Vec<FileItem>,
    pub path_to_index: AHashMap<PathBuf, usize>,
    pub git_workdir: Option<PathBuf>,
}

impl FileSync {
    fn new() -> Self {
        Self {
            files: Vec::new(),
            path_to_index: AHashMap::new(),
            git_workdir: None,
        }
    }

    /// Fast O(1) lookup using HashMap (15-18x faster than binary search)
    fn find_file_index(&self, path: &Path) -> Result<usize, usize> {
        self.path_to_index.get(path).copied().ok_or(0) // Return Err(0) to match binary_search signature
    }
}

impl FileItem {
    pub fn new(path: PathBuf, base_path: &Path, git_status: Option<Status>) -> Self {
        let relative_path = pathdiff::diff_paths(&path, base_path)
            .unwrap_or_else(|| path.clone())
            .to_string_lossy()
            .into_owned();

        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        let (size, modified) = match std::fs::metadata(&path) {
            Ok(metadata) => {
                let size = metadata.len();
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());

                (size, modified)
            }
            Err(_) => (0, 0),
        };

        let is_binary = detect_binary(&path, size);

        Self::new_raw(
            path,
            relative_path,
            name,
            size,
            modified,
            git_status,
            is_binary,
        )
    }

    pub fn update_frecency_scores(&mut self, tracker: &FrecencyTracker) -> Result<(), Error> {
        self.access_frecency_score = tracker.get_access_score(&self.path);
        self.modification_frecency_score =
            tracker.get_modification_score(self.modified, self.git_status);
        self.total_frecency_score = self.access_frecency_score + self.modification_frecency_score;

        Ok(())
    }

    /// Locks the tracker and updates frecensy score for one file. If need multiple files updates
    /// use `update_frecency_scores` instead.
    pub fn update_frecency_scores_global(&mut self) -> Result<(), Error> {
        let Some(ref frecency) = *FRECENCY.read().map_err(|_| Error::AcquireFrecencyLock)? else {
            return Ok(());
        };

        self.update_frecency_scores(frecency)
    }
}

pub struct FilePicker {
    base_path: PathBuf,
    sync_data: FileSync,
    is_scanning: Arc<AtomicBool>,
    scanned_files_count: Arc<AtomicUsize>,
    background_watcher: Option<BackgroundWatcher>,
    warmup_mmap_cache: bool,
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

    pub fn warmup_mmap_cache(&self) -> bool {
        self.warmup_mmap_cache
    }

    pub fn git_root(&self) -> Option<&Path> {
        self.sync_data.git_workdir.as_deref()
    }

    /// Get files in frecency order (already sorted, zero overhead).
    /// Files are always stored sorted by frecency in the sync_data.files Vec.
    pub fn get_files(&self) -> &[FileItem] {
        &self.sync_data.files
    }

    pub fn new(base_path: String) -> Result<Self, Error> {
        Self::new_with_options(base_path, false)
    }

    pub fn new_with_options(base_path: String, warmup_mmap_cache: bool) -> Result<Self, Error> {
        info!(
            "Initializing FilePicker with base_path: {}, warmup: {}",
            base_path, warmup_mmap_cache
        );
        let path = PathBuf::from(&base_path);
        if !path.exists() {
            error!("Base path does not exist: {}", base_path);
            return Err(Error::InvalidPath(path));
        }

        let scan_signal = Arc::new(AtomicBool::new(false));
        let synced_files_count = Arc::new(AtomicUsize::new(0));

        let picker = Self {
            base_path: path.clone(),
            sync_data: FileSync::new(),
            is_scanning: Arc::clone(&scan_signal),
            scanned_files_count: Arc::clone(&synced_files_count),
            background_watcher: None,
            warmup_mmap_cache,
        };

        // Background thread writes back into the global FILE_PICKER / FRECENCY.
        spawn_scan_and_watcher(
            path.clone(),
            Arc::clone(&scan_signal),
            Arc::clone(&synced_files_count),
            warmup_mmap_cache,
            None,
            None,
        );

        Ok(picker)
    }

    /// Create a new FilePicker and place it into the provided shared handle.
    ///
    /// Unlike [`new_with_options`], this does **not** touch the global
    /// `FILE_PICKER` / `FRECENCY` statics. The background scan thread
    /// and file-system watcher write into the provided `SharedPicker` and
    /// read frecency data from the provided `SharedFrecency`.
    ///
    /// Multiple independent instances can coexist in the same process.
    pub fn new_with_shared_state(
        base_path: String,
        warmup_mmap_cache: bool,
        shared_picker: SharedPicker,
        shared_frecency: SharedFrecency,
    ) -> Result<(), Error> {
        info!(
            "Initializing FilePicker with base_path: {}, warmup: {}",
            base_path, warmup_mmap_cache
        );
        let path = PathBuf::from(&base_path);
        if !path.exists() {
            error!("Base path does not exist: {}", base_path);
            return Err(Error::InvalidPath(path));
        }

        let scan_signal = Arc::new(AtomicBool::new(false));
        let synced_files_count = Arc::new(AtomicUsize::new(0));

        let picker = FilePicker {
            base_path: path.clone(),
            sync_data: FileSync::new(),
            is_scanning: Arc::clone(&scan_signal),
            scanned_files_count: Arc::clone(&synced_files_count),
            background_watcher: None,
            warmup_mmap_cache,
        };

        // Place the picker into the shared handle before spawning the
        // background thread so the thread can find it immediately.
        {
            let mut guard = shared_picker.write().map_err(|_| Error::AcquireItemLock)?;
            *guard = Some(picker);
        }

        spawn_scan_and_watcher(
            path.clone(),
            Arc::clone(&scan_signal),
            Arc::clone(&synced_files_count),
            warmup_mmap_cache,
            Some(shared_picker),
            Some(shared_frecency),
        );

        Ok(())
    }

    /// Perform fuzzy search on files with a pre-parsed query.
    ///
    /// The query should be parsed using `QueryParser::parse()` before calling this function.
    /// This allows the caller to handle location parsing and other preprocessing.
    ///
    /// # Arguments
    /// * `files` - Slice of files to search
    /// * `query` - The raw query string (used for max_typos calculation and debugging)
    /// * `parsed` - Pre-parsed query result (can be None for simple single-token queries)
    /// * `options` - Search options including pagination, threading, and scoring parameters
    ///
    /// # Returns
    /// SearchResult containing matched files, scores, and location information
    pub fn fuzzy_search<'a>(
        files: &'a [FileItem],
        query: &'a str,
        parsed: Option<FFFQuery<'a>>,
        options: FuzzySearchOptions<'a>,
    ) -> SearchResult<'a> {
        let max_threads = options.max_threads.max(1);
        debug!(
            ?query,
            parsed_is_some = parsed.is_some(),
            pagination = ?options.pagination,
            ?max_threads,
            current_file = ?options.current_file,
            "Fuzzy search",
        );

        let total_files = files.len();

        // Extract location from parsed query
        let location = parsed.as_ref().and_then(|p| p.location);

        // Get effective query for max_typos calculation (without location suffix)
        let effective_query = match &parsed {
            Some(p) => match &p.fuzzy_query {
                fff_query_parser::FuzzyQuery::Text(t) => *t,
                fff_query_parser::FuzzyQuery::Parts(parts) if !parts.is_empty() => parts[0],
                _ => query.trim(),
            },
            None => query.trim(),
        };

        // small queries with a large number of results can match absolutely everything
        let max_typos = (effective_query.len() as u16 / 4).clamp(2, 6);

        let context = ScoringContext {
            raw_query: query,
            parsed_query: parsed,
            project_path: options.project_path,
            max_typos,
            max_threads,
            current_file: options.current_file,
            last_same_query_match: options.last_same_query_match,
            combo_boost_score_multiplier: options.combo_boost_score_multiplier,
            min_combo_count: options.min_combo_count,
            pagination: options.pagination,
        };

        let time = std::time::Instant::now();

        let (items, scores, total_matched) = match_and_score_files(files, &context);

        debug!(
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

    pub fn get_scan_progress(&self) -> ScanProgress {
        let scanned_count = self.scanned_files_count.load(Ordering::Relaxed);
        let is_scanning = self.is_scanning.load(Ordering::Relaxed);
        ScanProgress {
            scanned_files_count: scanned_count,
            is_scanning,
        }
    }

    pub fn update_git_statuses(&mut self, status_cache: GitStatusCache) -> Result<(), Error> {
        self.update_git_statuses_with_frecency(status_cache, None)
    }

    /// Update git statuses for files, optionally using a specific frecency tracker
    /// instead of the global `FRECENCY` static.
    pub fn update_git_statuses_with_frecency(
        &mut self,
        status_cache: GitStatusCache,
        shared_frecency: Option<&SharedFrecency>,
    ) -> Result<(), Error> {
        debug!(
            statuses_count = status_cache.statuses_len(),
            "Updating git status",
        );

        // Read frecency from the shared handle if provided, otherwise from the global.
        if let Some(sf) = shared_frecency {
            let frecency = sf.read().map_err(|_| Error::AcquireFrecencyLock)?;
            status_cache
                .into_iter()
                .try_for_each(|(path, status)| -> Result<(), Error> {
                    if let Some(file) = self.get_mut_file_by_path(&path) {
                        file.git_status = Some(status);
                        if let Some(ref f) = *frecency {
                            file.update_frecency_scores(f)?;
                        }
                    } else {
                        error!(?path, "Couldn't update the git status for path");
                    }
                    Ok(())
                })?;
        } else {
            let frecency = FRECENCY.read().map_err(|_| Error::AcquireFrecencyLock)?;
            status_cache
                .into_iter()
                .try_for_each(|(path, status)| -> Result<(), Error> {
                    if let Some(file) = self.get_mut_file_by_path(&path) {
                        file.git_status = Some(status);
                        if let Some(frecency) = frecency.as_ref() {
                            file.update_frecency_scores(frecency)?;
                        }
                    } else {
                        error!(?path, "Couldn't update the git status for path");
                    }
                    Ok(())
                })?;
        }

        Ok(())
    }

    /// Fetches all the git statuses first and updates the global FILE_PICKER
    /// with the new statuses with the smallest possible lock time.
    pub fn refresh_git_status_global() -> Result<usize, Error> {
        let git_status = {
            let Some(ref picker) = *FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)? else {
                return Err(Error::FilePickerMissing)?;
            };

            debug!(
                "Refreshing git statuses for picker: {:?}",
                picker.git_root()
            );

            // we keep here readonly lock but allowing querying the index while it scan lasts
            GitStatusCache::read_git_status(
                picker.git_root(),
                StatusOptions::new()
                    .include_untracked(true)
                    .recurse_untracked_dirs(true)
                    // when manually refreshing git status we want to include all unmodified file
                    // to make sure that their status is correctly updated when user
                    // commited/stashed/removed changes
                    .include_unmodified(true)
                    .exclude_submodules(true),
            )
        };

        let mut file_picker = FILE_PICKER.write().map_err(|_| Error::AcquireItemLock)?;
        let picker = file_picker
            .as_mut()
            .ok_or_else(|| Error::FilePickerMissing)?;

        let statuses_count = if let Some(git_status) = git_status {
            let count = git_status.statuses_len();
            picker.update_git_statuses(git_status)?;

            count
        } else {
            0
        };

        Ok(statuses_count)
    }

    /// Instance-based variant of [`refresh_git_status_global`].
    ///
    /// Refreshes git statuses using the provided shared picker handle instead
    /// of the global `FILE_PICKER` static.
    pub fn refresh_git_status_shared(shared_picker: &SharedPicker) -> Result<usize, Error> {
        let git_status = {
            let guard = shared_picker.read().map_err(|_| Error::AcquireItemLock)?;
            let Some(ref picker) = *guard else {
                return Err(Error::FilePickerMissing);
            };

            debug!(
                "Refreshing git statuses for picker: {:?}",
                picker.git_root()
            );

            GitStatusCache::read_git_status(
                picker.git_root(),
                StatusOptions::new()
                    .include_untracked(true)
                    .recurse_untracked_dirs(true)
                    .include_unmodified(true)
                    .exclude_submodules(true),
            )
        };

        let mut guard = shared_picker.write().map_err(|_| Error::AcquireItemLock)?;
        let picker = guard.as_mut().ok_or(Error::FilePickerMissing)?;

        let statuses_count = if let Some(git_status) = git_status {
            let count = git_status.statuses_len();
            picker.update_git_statuses(git_status)?;
            count
        } else {
            0
        };

        Ok(statuses_count)
    }

    pub fn update_single_file_frecency(
        &mut self,
        file_path: impl AsRef<Path>,
        frecency_tracker: &FrecencyTracker,
    ) -> Result<(), Error> {
        if let Ok(index) = self.sync_data.find_file_index(file_path.as_ref())
            && let Some(file) = self.sync_data.files.get_mut(index)
        {
            file.update_frecency_scores(frecency_tracker)?;
        }

        Ok(())
    }

    pub fn get_file_by_path(&self, path: impl AsRef<Path>) -> Option<&FileItem> {
        self.sync_data
            .find_file_index(path.as_ref())
            .ok()
            .and_then(|index| self.sync_data.files.get(index))
    }

    pub fn get_mut_file_by_path(&mut self, path: impl AsRef<Path>) -> Option<&mut FileItem> {
        self.sync_data
            .find_file_index(path.as_ref())
            .ok()
            .and_then(|index| self.sync_data.files.get_mut(index))
    }

    /// Add a file to the picker's files in sorted order (used by background watcher)
    pub fn add_file_sorted(&mut self, file: FileItem) -> Option<&FileItem> {
        match self
            .sync_data
            .files
            .binary_search_by(|f| f.relative_path.cmp(&file.relative_path))
        {
            Ok(position) => {
                warn!(
                    "Trying to insert a file that already exists: {}",
                    file.relative_path
                );

                self.sync_data.files.get(position)
            }
            Err(position) => {
                self.sync_data.files.insert(position, file);
                self.sync_data.files.get(position)
            }
        }
    }

    #[tracing::instrument(skip(self), name = "timing_update", level = Level::DEBUG)]
    pub fn on_create_or_modify(&mut self, path: impl AsRef<Path> + Debug) -> Option<&FileItem> {
        let path = path.as_ref();
        match self.sync_data.find_file_index(path) {
            Ok(pos) => {
                // safe to read because we are in lock and binary search returned valid position
                let file = &mut self.sync_data.files[pos];

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

                        // TODO figure out if we actually need to remap the memory or invalidate
                        // mapping here because on linux and macos with the shared map opening it
                        // should be automatically available everywhere automatically which saves
                        // some time from doing extra remapping on every search
                        file.invalidate_mmap();
                    }
                }

                Some(file)
            }
            Err(pos) => {
                let file_item = FileItem::new(path.to_path_buf(), &self.base_path, None);
                self.sync_data.files.insert(pos, file_item);

                self.sync_data.files.get(pos)
            }
        }
    }

    pub fn remove_file_by_path(&mut self, path: impl AsRef<Path>) -> bool {
        match self.sync_data.find_file_index(path.as_ref()) {
            Ok(index) => {
                self.sync_data.files.remove(index);
                true
            }
            Err(_) => false,
        }
    }

    // TODO make this O(n)
    pub fn remove_all_files_in_dir(&mut self, dir: impl AsRef<Path>) -> usize {
        let dir_path = dir.as_ref();
        let initial_len = self.sync_data.files.len();

        self.sync_data
            .files
            .retain(|file| !file.path.starts_with(dir_path));

        initial_len - self.sync_data.files.len()
    }

    pub fn stop_background_monitor(&mut self) {
        if let Some(watcher) = self.background_watcher.take() {
            watcher.stop();
        }
    }

    pub fn trigger_rescan(&mut self) -> Result<(), Error> {
        if self.is_scanning.load(Ordering::Relaxed) {
            debug!("Scan already in progress, skipping trigger_rescan");
            return Ok(());
        }

        self.is_scanning.store(true, Ordering::Relaxed);
        self.scanned_files_count.store(0, Ordering::Relaxed);

        let scan_result = scan_filesystem(&self.base_path, &self.scanned_files_count, None);
        match scan_result {
            Ok(sync) => {
                info!(
                    "Filesystem scan completed: found {} files",
                    sync.files.len()
                );

                self.sync_data = sync;

                if self.warmup_mmap_cache {
                    // Warmup in background to avoid blocking
                    let files = self.sync_data.files.clone();
                    std::thread::spawn(move || {
                        warmup_mmaps(&files);
                    });
                }
            }
            Err(error) => error!(?error, "Failed to scan file system"),
        }

        self.is_scanning.store(false, Ordering::Relaxed);
        Ok(())
    }

    pub fn is_scan_active(&self) -> bool {
        self.is_scanning.load(Ordering::Relaxed)
    }
}

#[allow(unused)]
#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub scanned_files_count: usize,
    pub is_scanning: bool,
}

fn spawn_scan_and_watcher(
    base_path: PathBuf,
    scan_signal: Arc<AtomicBool>,
    synced_files_count: Arc<AtomicUsize>,
    warmup_mmap_cache: bool,
    shared_picker: Option<SharedPicker>,
    shared_frecency: Option<SharedFrecency>,
) {
    std::thread::spawn(move || {
        scan_signal.store(true, Ordering::Relaxed);
        info!("Starting initial file scan");

        let mut git_workdir = None;
        match scan_filesystem(&base_path, &synced_files_count, shared_frecency.as_ref()) {
            Ok(sync) => {
                info!(
                    "Initial filesystem scan completed: found {} files",
                    sync.files.len()
                );

                git_workdir = sync.git_workdir.clone();

                // Write results into the provided shared handle, or fall back to
                // the global FILE_PICKER when running in global mode.
                let write_result = if let Some(ref sp) = shared_picker {
                    sp.write().ok().map(|mut guard| {
                        if let Some(ref mut picker) = *guard {
                            picker.sync_data = sync;
                        }
                    })
                } else {
                    crate::FILE_PICKER.write().ok().map(|mut guard| {
                        if let Some(ref mut picker) = *guard {
                            picker.sync_data = sync;
                        }
                    })
                };

                if write_result.is_none() {
                    error!("Failed to write scan results into picker");
                }

                // OPTIMIZATION: Warmup mmap cache in background to avoid blocking first grep.
                // The aggressive parallel warmup was causing cache thrashing and delaying
                // initial searches. Now it runs async and doesn't block.
                if warmup_mmap_cache {
                    let picker_clone = if let Some(ref sp) = shared_picker {
                        sp.read()
                            .ok()
                            .and_then(|g| g.as_ref().map(|p| p.sync_data.files.clone()))
                    } else {
                        crate::FILE_PICKER
                            .read()
                            .ok()
                            .and_then(|g| g.as_ref().map(|p| p.sync_data.files.clone()))
                    };

                    if let Some(files) = picker_clone {
                        warmup_mmaps(&files);
                    }
                }

                if write_result.is_none() {
                    error!("Failed to write scan results into picker");
                }
            }
            Err(e) => {
                error!("Initial scan failed: {:?}", e);
            }
        }
        scan_signal.store(false, Ordering::Relaxed);

        match BackgroundWatcher::new(
            base_path,
            git_workdir,
            shared_picker.clone(),
            shared_frecency.clone(),
        ) {
            Ok(watcher) => {
                info!("Background file watcher initialized successfully");

                let write_result = if let Some(ref sp) = shared_picker {
                    sp.write().ok().map(|mut guard| {
                        if let Some(ref mut picker) = *guard {
                            picker.background_watcher = Some(watcher);
                        }
                    })
                } else {
                    crate::FILE_PICKER.write().ok().map(|mut guard| {
                        if let Some(ref mut picker) = *guard {
                            picker.background_watcher = Some(watcher);
                        }
                    })
                };

                if write_result.is_none() {
                    error!("Failed to store background watcher in picker");
                }
            }
            Err(e) => {
                error!("Failed to initialize background file watcher: {:?}", e);
            }
        }

        // the debouncer keeps running in its own thread
    });
}

/// Pre-populate mmap caches for all eligible files so the first grep search
/// doesn't pay the mmap creation + page fault cost.
///
/// Each file is mmap'd and a single byte is read to trigger the page fault.
/// This runs in parallel using rayon.
fn warmup_mmaps(files: &[FileItem]) {
    let warmup_start = std::time::Instant::now();
    let warmed = std::sync::atomic::AtomicUsize::new(0);

    files.par_iter().for_each(|file| {
        if file.is_binary || file.size == 0 {
            return;
        }

        if let Some(mmap) = file.get_mmap() {
            // Read the first byte to trigger the initial page fault, which
            // causes the kernel to start readahead for subsequent pages.
            // This is cheaper than madvise and portable across all platforms.
            let _ = std::hint::black_box(mmap.first());

            warmed.fetch_add(1, Ordering::Relaxed);
        }
    });

    let warmed_count = warmed.load(Ordering::Relaxed);
    info!(
        "Mmap warmup completed: {warmed_count}/{} files in {:?}",
        files.len(),
        warmup_start.elapsed()
    );
}

fn scan_filesystem(
    base_path: &Path,
    synced_files_count: &Arc<AtomicUsize>,
    shared_frecency: Option<&SharedFrecency>,
) -> Result<FileSync, Error> {
    use ignore::{WalkBuilder, WalkState};
    use std::thread;

    let scan_start = std::time::Instant::now();
    info!("SCAN: Starting parallel filesystem scan and git status");

    // run separate thread for git status because it effectively does another separate file
    // traversal which could be pretty slow on large repos (in general 300-500ms)
    thread::scope(|s| {
        let git_handle = s.spawn(|| {
            let git_workdir = Repository::discover(base_path)
                .ok()
                .and_then(|repo| repo.workdir().map(Path::to_path_buf));

            if let Some(ref git_dir) = git_workdir {
                debug!("Git repository found at: {}", git_dir.display());
            } else {
                debug!("No git repository found for path: {}", base_path.display());
            }

            let status_cache = GitStatusCache::read_git_status(
                git_workdir.as_deref(),
                // do not include unmodified here to avoid extra cost
                // we are treating all missing files as unmodified
                StatusOptions::new()
                    .include_untracked(true)
                    .recurse_untracked_dirs(true)
                    .exclude_submodules(true),
            );

            (git_workdir, status_cache)
        });

        let walker = WalkBuilder::new(base_path)
            .hidden(false)
            .git_ignore(true)
            .git_exclude(true)
            .git_global(true)
            .ignore(true)
            .follow_links(false)
            .build_parallel();

        let walker_start = std::time::Instant::now();
        info!("SCAN: Starting file walker");

        let files = Arc::new(std::sync::Mutex::new(Vec::new()));
        walker.run(|| {
            let files = Arc::clone(&files);
            let counter = Arc::clone(synced_files_count);
            let base_path = base_path.to_path_buf();

            Box::new(move |result| {
                if let Ok(entry) = result
                    && entry.file_type().is_some_and(|ft| ft.is_file())
                {
                    let path = entry.path();

                    if is_git_file(path) {
                        return WalkState::Continue;
                    }

                    let file_item = FileItem::new(
                        path.to_path_buf(),
                        &base_path,
                        None, // Git status will be added after join
                    );

                    if let Ok(mut files_vec) = files.lock() {
                        files_vec.push(file_item);
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                }
                WalkState::Continue
            })
        });

        let mut files = Arc::try_unwrap(files).unwrap().into_inner().unwrap();
        let walker_time = walker_start.elapsed();
        info!("SCAN: File walking completed in {:?}", walker_time);

        let (git_workdir, git_cache) = git_handle.join().map_err(|_| {
            error!("Failed to join git status thread");
            Error::ThreadPanic
        })?;

        let frecency = if let Some(sf) = shared_frecency {
            sf.read().map_err(|_| Error::AcquireFrecencyLock)?
        } else {
            FRECENCY.read().map_err(|_| Error::AcquireFrecencyLock)?
        };
        files
            .par_iter_mut()
            .try_for_each(|file| -> Result<(), Error> {
                if let Some(git_cache) = &git_cache {
                    file.git_status = git_cache.lookup_status(&file.path);
                }

                if let Some(frecency) = frecency.as_ref() {
                    file.update_frecency_scores(frecency)?;
                }

                Ok(())
            })?;

        let total_time = scan_start.elapsed();
        info!(
            "SCAN: Total scan time {:?} for {} files",
            total_time,
            files.len()
        );

        // Sort files by frecency (descending) for optimal grep performance
        info!("SCAN: Sorting files by frecency");
        let sort_start = std::time::Instant::now();

        files.par_sort_unstable_by(|a, b| {
            b.total_frecency_score
                .cmp(&a.total_frecency_score)
                .then(b.modified.cmp(&a.modified))
        });

        // Build path_to_index HashMap for O(1) lookups
        let path_to_index: AHashMap<PathBuf, usize> = files
            .iter()
            .enumerate()
            .map(|(idx, file)| (file.path.clone(), idx))
            .collect();

        info!(
            "SCAN: Sort and index build completed in {:?}",
            sort_start.elapsed()
        );

        Ok(FileSync {
            files,
            path_to_index,
            git_workdir,
        })
    })
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
