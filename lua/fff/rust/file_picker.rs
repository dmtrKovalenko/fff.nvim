use crate::background_watcher::BackgroundWatcher;
use crate::error::Error;
use crate::frecency::FrecencyTracker;
use crate::git::GitStatusCache;
use crate::location::parse_location;
use crate::score::match_and_score_files;
use crate::types::{FileItem, ScoringContext, SearchResult};
use git2::{Repository, Status, StatusOptions};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::SystemTime;
use tracing::{debug, error, info, warn};

use crate::{FILE_PICKER, FRECENCY};

#[derive(Debug, Clone)]
struct FileSync {
    pub files: Vec<FileItem>,
    pub git_workdir: Option<PathBuf>,
}

impl FileSync {
    fn new() -> Self {
        Self {
            files: Vec::new(),
            git_workdir: None,
        }
    }

    fn find_file_index(&self, path: &Path) -> Result<usize, usize> {
        self.files
            .binary_search_by(|file| file.path.as_path().cmp(path))
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

        Self {
            path,
            relative_path,
            file_name: name,
            size,
            modified,
            access_frecency_score: 0,
            modification_frecency_score: 0,
            total_frecency_score: 0,
            git_status,
        }
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
    pub fn git_root(&self) -> Option<&Path> {
        self.sync_data.git_workdir.as_deref()
    }

    pub fn get_files(&self) -> &[FileItem] {
        &self.sync_data.files
    }

    pub fn new(base_path: String) -> Result<Self, Error> {
        info!("Initializing FilePicker with base_path: {}", base_path);
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
        };

        spawn_scan_and_watcher(
            path.clone(),
            Arc::clone(&scan_signal),
            Arc::clone(&synced_files_count),
        );

        Ok(picker)
    }

    pub fn fuzzy_search<'a>(
        files: &'a [FileItem],
        query: &'a str,
        max_results: usize,
        max_threads: usize,
        current_file: Option<&'a str>,
        reverse_order: bool,
    ) -> SearchResult<'a> {
        let max_threads = max_threads.max(1);
        debug!(
            ?query,
            ?max_results,
            ?max_threads,
            ?current_file,
            "Fuzzy search",
        );

        let total_files = files.len();
        let (query, location) = parse_location(query);

        // small queries with a large number of results can match absolutely everything
        let max_typos = (query.len() as u16 / 4).clamp(2, 6);
        let context = ScoringContext {
            query,
            max_typos,
            max_threads,
            current_file,
            max_results,
            reverse_order,
        };

        let time = std::time::Instant::now();
        let (items, scores, total_matched) = match_and_score_files(files, &context);

        debug!(
            ?query,
            completed_in = ?time.elapsed(),
            top_position = ?items.first(),
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

    pub fn update_git_statuses(
        &mut self,
        status_cache: Option<GitStatusCache>,
    ) -> Result<(), Error> {
        let Some(status_cache) = status_cache else {
            return Ok(());
        };

        debug!(
            statuses_count = status_cache.statuses_len(),
            "Updating git status",
        );

        let frecency = FRECENCY.read().map_err(|_| Error::AcquireFrecencyLock)?;
        status_cache
            .into_iter()
            .try_for_each(|(path, status)| -> Result<(), Error> {
                if let Some(file) = self.get_mut_file_by_path(&path) {
                    file.git_status = Some(status);

                    if let Some(frecency) = frecency.as_ref() {
                        file.update_frecency_scores(frecency)?;
                    }
                }

                Ok(())
            })?;

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

        let statuses_count = git_status.as_ref().map_or(0, |cache| cache.statuses_len());
        picker.update_git_statuses(git_status)?;

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

    pub fn on_create_or_modify(&mut self, path: impl AsRef<Path>) -> Option<&FileItem> {
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

        if let Ok(sync) = scan_filesystem(&self.base_path, &self.scanned_files_count) {
            info!(
                "Filesystem scan completed: found {} files",
                sync.files.len()
            );
            self.sync_data = sync
        } else {
            warn!("Filesystem scan failed");
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
) {
    std::thread::spawn(move || {
        scan_signal.store(true, Ordering::Relaxed);
        info!("Starting initial file scan");

        let mut git_workdir = None;
        match scan_filesystem(&base_path, &synced_files_count) {
            Ok(sync) => {
                info!(
                    "Initial filesystem scan completed: found {} files",
                    sync.files.len()
                );

                git_workdir = sync.git_workdir.clone();
                if let Ok(mut file_picker_guard) = crate::FILE_PICKER.write()
                    && let Some(ref mut picker) = *file_picker_guard
                {
                    picker.sync_data = sync;
                }
            }
            Err(e) => {
                error!("Initial scan failed: {:?}", e);
            }
        }
        scan_signal.store(false, Ordering::Relaxed);

        match BackgroundWatcher::new(base_path, git_workdir) {
            Ok(watcher) => {
                info!("Background file watcher initialized successfully");

                if let Ok(mut file_picker_guard) = crate::FILE_PICKER.write()
                    && let Some(ref mut picker) = *file_picker_guard
                {
                    picker.background_watcher = Some(watcher);
                }
            }
            Err(e) => {
                error!("Failed to initialize background file watcher: {:?}", e);
            }
        }

        // the debouncer keeps running in its own thread
    });
}

fn scan_filesystem(
    base_path: &Path,
    synced_files_count: &Arc<AtomicUsize>,
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

        let frecency = FRECENCY.read().map_err(|_| Error::AcquireFrecencyLock)?;
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

        files.par_sort_unstable_by(|a, b| a.path.cmp(&b.path));
        Ok(FileSync { files, git_workdir })
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
