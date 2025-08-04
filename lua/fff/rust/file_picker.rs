use crate::error::Error;
use crate::file_key::FileKey;
use crate::git::{format_git_status, GitStatusCache};
use crate::score::match_and_score_files;
use crate::types::{FileItem, Score, ScoringContext, SearchResult};
use git2::{Repository, Status, StatusOptions};
use ignore::{WalkBuilder, WalkState};
use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, DebouncedEvent};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, RwLock,
};
use std::thread;
use std::time::{Duration, SystemTime};
use tracing::{debug, error, info, warn};

use crate::FRECENCY;

#[derive(Debug, Clone)]
struct FileSync {
    files: Vec<FileItem>,
    last_update: SystemTime,
    git_status_cache: Option<GitStatusCache>,
    scan_generation: u64,
}

type Debouncer = notify_debouncer_full::Debouncer<
    notify::RecommendedWatcher,
    notify_debouncer_full::RecommendedCache,
>;

impl FileSync {
    fn new() -> Self {
        Self {
            files: Vec::new(),
            last_update: SystemTime::UNIX_EPOCH,
            git_status_cache: None,
            scan_generation: 0,
        }
    }

    fn update_files(&mut self, mut files: Vec<FileItem>, git_status_cache: Option<GitStatusCache>) {
        files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        self.files = files;
        self.git_status_cache = git_status_cache;
        self.last_update = SystemTime::now();
        self.scan_generation = self.scan_generation.wrapping_add(1);
    }

    fn contains_path(&self, path: &str) -> bool {
        self.files
            .binary_search_by(|file| file.relative_path.as_str().cmp(path))
            .is_ok()
    }

    fn find_file_index(&self, path: &str) -> Result<usize, usize> {
        self.files
            .binary_search_by(|file| file.relative_path.as_str().cmp(path))
    }

    fn insert_file_sorted(&mut self, file: FileItem) {
        match self
            .files
            .binary_search_by(|f| f.relative_path.cmp(&file.relative_path))
        {
            Ok(_) => {
                warn!(
                    "Trying to insert a file that already exists: {}",
                    file.relative_path
                );
            }
            Err(pos) => {
                self.files.insert(pos, file);
                self.scan_generation = self.scan_generation.wrapping_add(1);
            }
        }
    }

    /// Remove file by path using binary search
    fn remove_file_by_path(&mut self, path: &str) -> bool {
        match self.find_file_index(path) {
            Ok(index) => {
                self.files.remove(index);
                self.scan_generation = self.scan_generation.wrapping_add(1);
                true
            }
            Err(_) => false,
        }
    }
}

impl FileItem {
    fn new(path: PathBuf, base_path: &Path, git_status: Option<Status>) -> Self {
        let relative_path = pathdiff::diff_paths(&path, base_path)
            .unwrap_or_else(|| path.clone())
            .to_string_lossy()
            .into_owned();

        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        let extension = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        let directory = match Path::new(&relative_path).parent() {
            Some(parent) if parent != Path::new(".") && !parent.as_os_str().is_empty() => {
                parent.to_string_lossy().into_owned()
            }
            _ => String::new(),
        };

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
            extension,
            directory,
            size,
            modified,
            access_frecency_score: 0,
            modification_frecency_score: 0,
            total_frecency_score: 0,
            git_status,
            is_current_file: false,
        }
    }

    fn update_frecency_scores(&mut self) {
        if let Ok(frecency) = FRECENCY.read() {
            if let Some(ref tracker) = *frecency {
                let file_key = FileKey::from(&*self);
                self.access_frecency_score = tracker.get_access_score(&file_key);
                self.modification_frecency_score = tracker
                    .get_modification_score(self.modified, format_git_status(self.git_status));
                self.total_frecency_score =
                    self.access_frecency_score + self.modification_frecency_score;
            }
        }
    }
}

impl From<&FileItem> for FileKey {
    fn from(file: &FileItem) -> Self {
        FileKey {
            path: file.relative_path.clone(),
        }
    }
}

pub struct FilePicker {
    base_path: PathBuf,
    git_workdir: Option<PathBuf>,
    sync_data: Arc<RwLock<FileSync>>,
    is_scanning: Arc<AtomicBool>,
    _debouncer: Arc<Mutex<Option<Debouncer>>>,
}

impl std::fmt::Debug for FilePicker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilePicker")
            .field("base_path", &self.base_path)
            .field("git_workdir", &self.git_workdir)
            .finish_non_exhaustive()
    }
}

impl FilePicker {
    pub fn new(base_path: String) -> Result<Self, Error> {
        info!("Initializing FilePicker with base_path: {}", base_path);
        let path = PathBuf::from(&base_path);
        if !path.exists() {
            error!("Base path does not exist: {}", base_path);
            return Err(Error::InvalidPath(path));
        }

        let git_workdir = Repository::discover(&path)
            .ok()
            .and_then(|repo| repo.workdir().map(Path::to_path_buf));

        if let Some(ref git_dir) = git_workdir {
            debug!("Git repository found at: {}", git_dir.display());
        } else {
            debug!("No git repository found for path: {}", base_path);
        }

        let sync_data = Arc::new(RwLock::new(FileSync::new()));
        let scan_signal = Arc::new(AtomicBool::new(false));
        let debouncer_holder = Arc::new(Mutex::new(None));

        let picker = Self {
            base_path: path.clone(),
            git_workdir: git_workdir.clone(),
            sync_data: Arc::clone(&sync_data),
            is_scanning: Arc::clone(&scan_signal),
            _debouncer: Arc::clone(&debouncer_holder),
        };

        spawn_async_initialization(path, git_workdir, sync_data, scan_signal, debouncer_holder);

        Ok(picker)
    }

    pub fn fuzzy_search(
        &self,
        query: &str,
        max_results: usize,
        max_threads: usize,
        current_file: Option<&String>,
    ) -> SearchResult {
        let max_threads = max_threads.max(1); // Ensure at least 1 to avoid neo_frizbee division by zero

        debug!(
            "Fuzzy search: query='{}', max_results={}, max_threads={}, current_file={:?}",
            query, max_results, max_threads, current_file
        );

        let time = std::time::Instant::now();
        let sync_data = self.sync_data.read().unwrap();
        let total_files = sync_data.files.len();

        // small queries with a large number of results can match absolutely everything
        let max_typos = (query.len() as u16 / 4).clamp(2, 6);
        let context = ScoringContext {
            query,
            max_typos,
            max_threads,
            current_file,
        };

        let scored_indices = match_and_score_files(&sync_data.files, &context);
        let total_matched = scored_indices.len();

        let mut scored_files: Vec<(FileItem, Score)> = scored_indices
            .into_par_iter()
            .map(|(idx, score)| (sync_data.files[idx].clone(), score))
            .collect();

        scored_files.par_sort_unstable_by(|a, b| {
            b.1.total
                .cmp(&a.1.total)
                .then_with(|| b.0.modified.cmp(&a.0.modified))
        });

        scored_files.truncate(max_results);

        let (items, scores): (Vec<FileItem>, Vec<Score>) = scored_files.into_iter().unzip();

        debug!(
            "Fuzzy search completed: found {} results for query '{}', total_matched={}, total_files={}, top result {:?}",
            items.len(),
            query,
            total_matched,
            total_files,
            items.first()
        );

        debug!("Total search time: {:?}", time.elapsed());
        SearchResult {
            items,
            scores,
            total_matched,
            total_files,
        }
    }

    pub fn get_cached_files(&self) -> Vec<FileItem> {
        self.sync_data.read().unwrap().files.clone()
    }

    pub fn get_scan_progress(&self) -> ScanProgress {
        let sync_data = self.sync_data.read().unwrap();
        let is_scanning = self.is_scanning.load(Ordering::Relaxed);
        ScanProgress {
            total_files: sync_data.files.len(),
            scanned_files: sync_data.files.len(),
            is_scanning,
        }
    }

    pub fn refresh_git_status(&self) -> Vec<FileItem> {
        let sync_data: &Arc<RwLock<FileSync>> = &self.sync_data;
        let git_workdir = self.git_workdir.as_deref();
        let new_git_status_cache = GitStatusCache::read_git_status(git_workdir);

        if let Ok(mut sync_data_write) = sync_data.write() {
            sync_data_write.git_status_cache = new_git_status_cache.clone();

            for file in &mut sync_data_write.files {
                file.git_status = new_git_status_cache
                    .as_ref()
                    .and_then(|git| git.lookup_status(&file.path));

                file.update_frecency_scores();
            }
        }

        self.get_cached_files()
    }

    pub fn stop_background_monitor(&self) -> Result<(), Error> {
        if let Ok(mut debouncer_guard) = self._debouncer.lock() {
            if let Some(debouncer) = debouncer_guard.take() {
                debouncer.stop_nonblocking();
                info!("File watcher stopped successfully");
            }
        }
        Ok(())
    }

    pub fn trigger_rescan(&self) -> Result<(), Error> {
        if self.is_scanning.load(Ordering::Relaxed) {
            debug!("Scan already in progress, skipping trigger_rescan");
            return Ok(());
        }

        info!("is_scanning = TRUE (manual rescan starting)");
        self.is_scanning.store(true, Ordering::Relaxed);

        let base_path = self.base_path.clone();
        let git_workdir = self.git_workdir.clone();
        let sync_data = Arc::clone(&self.sync_data);
        let scan_signal = Arc::clone(&self.is_scanning);

        thread::spawn(move || {
            debug!("Background scan thread started");
            if let Ok((files, git_cache)) = scan_filesystem(&base_path, git_workdir.as_ref()) {
                info!("Filesystem scan completed: found {} files", files.len());
                if let Ok(mut data) = sync_data.write() {
                    data.update_files(files, git_cache);
                    debug!("File cache updated successfully");
                }
            } else {
                warn!("Filesystem scan failed");
            }

            scan_signal.store(false, Ordering::Relaxed);
            info!("is_scanning = FALSE (manual rescan completed)");
        });

        Ok(())
    }

    pub fn is_scan_active(&self) -> bool {
        self.is_scanning.load(Ordering::Relaxed)
    }
}

#[allow(unused)]
#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub total_files: usize,
    pub scanned_files: usize,
    pub is_scanning: bool,
}

fn spawn_async_initialization(
    base_path: PathBuf,
    git_workdir: Option<PathBuf>,
    sync_data: Arc<RwLock<FileSync>>,
    scan_signal: Arc<AtomicBool>,
    debouncer_holder: Arc<Mutex<Option<Debouncer>>>,
) {
    thread::spawn(move || {
        scan_signal.store(true, Ordering::Relaxed);
        info!("Starting async initialization for file picker");

        match scan_filesystem(&base_path, git_workdir.as_ref()) {
            Ok((files, git_cache)) => {
                info!(
                    "Initial filesystem scan completed: found {} files",
                    files.len()
                );
                if let Ok(mut data) = sync_data.write() {
                    data.update_files(files, git_cache);
                    debug!("Initial file cache updated successfully");
                }
            }
            Err(e) => {
                error!("Initial scan failed: {:?}", e);
            }
        }
        scan_signal.store(false, Ordering::Relaxed);

        match create_file_watcher_sync(base_path, git_workdir, Arc::clone(&sync_data)) {
            Ok(debouncer) => {
                if let Ok(mut holder) = debouncer_holder.lock() {
                    *holder = Some(debouncer);
                    info!("File watcher setup completed successfully");
                } else {
                    error!("Failed to store debouncer - mutex poisoned");
                }
            }
            Err(e) => {
                error!("Failed to create file watcher: {:?}", e);
            }
        }
    });
}

fn create_file_watcher_sync(
    base_path: PathBuf,
    git_workdir: Option<PathBuf>,
    sync_data: Arc<RwLock<FileSync>>,
) -> Result<Debouncer, Error> {
    let mut debouncer = new_debouncer(Duration::from_millis(500), None, {
        let sync_data = Arc::clone(&sync_data);
        let base_path = base_path.clone();
        let git_workdir = git_workdir.clone();

        move |result: DebounceEventResult| match result {
            Ok(events) => {
                handle_debounced_events(events, &sync_data, &base_path, &git_workdir);
            }
            Err(errors) => {
                error!("File watcher errors: {:?}", errors);
            }
        }
    })?;

    if let Err(e) = debouncer.watch(&base_path, RecursiveMode::Recursive) {
        error!(
            "Failed to start watching path: {}, error {e:?}",
            base_path.display(),
        );
        return Err(e.into());
    }

    info!("File watcher started for path: {}", base_path.display());
    Ok(debouncer)
}

fn handle_debounced_events(
    events: Vec<DebouncedEvent>,
    sync_data: &Arc<RwLock<FileSync>>,
    base_path: &Path,
    git_workdir: &Option<PathBuf>,
) {
    let mut affected_paths = Vec::new();
    for event in events {
        let relevant_paths: Vec<_> = event
            .paths
            .iter()
            .filter_map(|path| {
                let relative_path = pathdiff::diff_paths(path, base_path)?;

                let Ok(sync_read) = sync_data.read() else {
                    return None;
                };

                let relative_str = relative_path.to_string_lossy();

                if sync_read.contains_path(&relative_str) {
                    return Some(path.clone());
                }

                match event.event.kind {
                    EventKind::Create(_) => {
                        if should_add_new_file(path, git_workdir.as_ref()) {
                            Some(path.clone())
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            })
            .collect();

        if relevant_paths.is_empty() {
            continue; // No relevant paths to process
        }

        debug!(?event, "File watcher event");
        match event.event.kind {
            EventKind::Create(_) => {
                handle_create_events(&relevant_paths, sync_data, base_path, git_workdir.as_ref());
                affected_paths.extend(relevant_paths);
            }
            EventKind::Modify(_) => {
                affected_paths.extend(relevant_paths);
            }
            EventKind::Remove(_) => {
                remove_paths_from_index(relevant_paths, sync_data, base_path);
            }
            _ => {
                affected_paths.extend(relevant_paths);
            }
        }
    }

    if !affected_paths.is_empty() {
        update_git_status_for_paths(sync_data, git_workdir, base_path, &affected_paths);
    }
}

fn should_add_new_file(path: &Path, git_workdir: Option<&PathBuf>) -> bool {
    if is_git_file(path) {
        return false;
    }

    if !path.is_file() {
        return false;
    }

    if let Some(git_workdir) = git_workdir {
        if let Ok(repo) = Repository::open(git_workdir) {
            if repo.is_path_ignored(path).unwrap_or(false) {
                return false;
            }
        }
    }

    true
}

fn handle_create_events(
    paths: &[PathBuf],
    sync_data: &Arc<RwLock<FileSync>>,
    base_path: &Path,
    git_workdir: Option<&PathBuf>,
) {
    let repo = git_workdir.as_ref().and_then(|p| Repository::open(p).ok());
    if let Ok(mut sync_write) = sync_data.write() {
        for path in paths {
            if repo
                .as_ref()
                .is_some_and(|repo| repo.is_path_ignored(path).unwrap_or(false))
            {
                debug!("Ignoring file {} due to gitignore rules", path.display());
                continue;
            }

            let mut file_item = FileItem::new(path.clone(), base_path, None);
            file_item.update_frecency_scores();
            sync_write.insert_file_sorted(file_item);
        }
    }
}

fn remove_paths_from_index(
    paths: Vec<PathBuf>,
    sync_data: &Arc<RwLock<FileSync>>,
    base_path: &Path,
) {
    if let Ok(mut sync_write) = sync_data.write() {
        for path in paths {
            if let Some(relative_path) = pathdiff::diff_paths(path, base_path) {
                let relative_str = relative_path.to_string_lossy();
                sync_write.remove_file_by_path(&relative_str);
            }
        }
    }
}

fn scan_filesystem(
    base_path: &Path,
    git_workdir: Option<&PathBuf>,
) -> Result<(Vec<FileItem>, Option<GitStatusCache>), Error> {
    let scan_start = std::time::Instant::now();
    let git_workdir = git_workdir.map(|p| p.as_path());
    info!("SCAN: Starting parallel filesystem scan and git status");

    // run separate thread for git status because it effectively does another separate file
    // traversal which could be pretty slow on large repos (in general 300-500ms)
    thread::scope(|s| {
        let git_handle = s.spawn(|| GitStatusCache::read_git_status(git_workdir));

        let walker = WalkBuilder::new(base_path)
            .hidden(false)
            .git_ignore(true)
            .git_exclude(true)
            .git_global(true)
            .ignore(true)
            .follow_links(false)
            .sort_by_file_name(std::cmp::Ord::cmp)
            .build_parallel();

        let walker_start = std::time::Instant::now();
        info!("SCAN: Starting file walker");

        let files = Arc::new(std::sync::Mutex::new(Vec::new()));
        walker.run(|| {
            let files = Arc::clone(&files);
            let base_path = base_path.to_path_buf();

            Box::new(move |result| {
                if let Ok(entry) = result {
                    if entry.file_type().is_some_and(|ft| ft.is_file()) {
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
                        }
                    }
                }
                WalkState::Continue
            })
        });

        let mut files = Arc::try_unwrap(files).unwrap().into_inner().unwrap();
        let walker_time = walker_start.elapsed();
        info!("SCAN: File walking completed in {:?}", walker_time);

        let git_cache = git_handle.join().map_err(|_| {
            error!("Failed to join git status thread");
            Error::ThreadPanic
        })?;

        if let Some(git_cache) = &git_cache {
            files.par_iter_mut().for_each(|file| {
                file.git_status = git_cache.lookup_status(&file.path);
                file.update_frecency_scores();
            });
        }

        let total_time = scan_start.elapsed();
        info!(
            "SCAN: Total scan time {:?} for {} files",
            total_time,
            files.len()
        );

        Ok((files, git_cache))
    })
}

fn update_git_status_for_paths(
    sync_data: &Arc<RwLock<FileSync>>,
    git_workdir: &Option<PathBuf>,
    base_path: &Path,
    affected_paths: &[PathBuf],
) {
    let Some(git_workdir) = git_workdir else {
        return;
    };

    let Ok(repo) = Repository::open(git_workdir) else {
        return;
    };

    let mut status_options = StatusOptions::new();
    status_options.include_untracked(true);
    status_options.include_ignored(false);

    for path in affected_paths {
        if let Some(relative_path) = pathdiff::diff_paths(path, base_path) {
            let path_str = relative_path.to_string_lossy();
            status_options.pathspec(&*path_str);
        }
    }

    let Ok(statuses) = repo.statuses(Some(&mut status_options)) else {
        error!(
            "Failed to get git statuses for affected paths: {:?}",
            affected_paths
        );
        return;
    };

    if let Ok(mut sync_write) = sync_data.write() {
        for status_entry in statuses.iter() {
            let Some(file_path) = status_entry.path() else {
                continue;
            };

            if let Ok(index) = sync_write.find_file_index(file_path) {
                sync_write.files[index].git_status = Some(status_entry.status());
                sync_write.files[index].update_frecency_scores();
            }
        }
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

impl Drop for FilePicker {
    fn drop(&mut self) {
        info!("FilePicker is being dropped, stopping file watcher");

        if let Ok(mut debouncer_guard) = self._debouncer.lock() {
            if let Some(debouncer) = debouncer_guard.take() {
                debouncer.stop();
                info!("File watcher stopped successfully");
            }
        } else {
            error!("Failed to acquire debouncer lock during drop");
        }
    }
}
