use crate::error::Error;
use crate::file_picker::{FFFMode, FilePicker};
use crate::git::GitStatusCache;
use crate::shared::{SharedFrecency, SharedPicker};
use crate::sort_buffer::sort_with_buffer;
use git2::Repository;
use notify::event::{AccessKind, AccessMode};
use notify::{Config, EventKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, DebouncedEvent, NoCache, new_debouncer_opt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use tracing::{Level, debug, error, info, warn};

type Debouncer = notify_debouncer_full::Debouncer<notify::RecommendedWatcher, NoCache>;

/// Owns the file-system watcher and guarantees that all background threads
/// are fully joined before `stop()` / `Drop` returns.
///
/// Architecture:
///   - The debouncer (and its internal watcher) live inside an **owner thread**
///     that we spawn and hold the `JoinHandle` for.
///   - `stop()` sets a flag, unparks the owner thread, and **joins** it.
///   - Inside the owner thread, `Debouncer::stop()` is called which joins the
///     debouncer's event-processing thread.
///   - On Windows an additional short sleep is added after `Debouncer::stop()`
///     because `notify`'s `ReadDirectoryChangesWatcher` discards its thread
///     `JoinHandle`, so we cannot join it directly. The watcher's `Drop` does
///     signal the thread via semaphore so it exits almost immediately, but we
///     need to give the OS a moment to reclaim it.
pub struct BackgroundWatcher {
    stop_signal: Arc<AtomicBool>,
    owner_thread: Option<std::thread::JoinHandle<()>>,
}

const DEBOUNCE_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_PATHS_THRESHOLD: usize = 1024;
/// Minimum seconds between frecency tracks of the same file in AI mode.
/// Prevents score inflation from rapid burst edits by AI agents.
const AI_MODE_COOLDOWN_SECS: u64 = 5 * 60;

impl BackgroundWatcher {
    pub fn new(
        base_path: PathBuf,
        git_workdir: Option<PathBuf>,
        shared_picker: SharedPicker,
        shared_frecency: SharedFrecency,
        mode: FFFMode,
        watch_dirs: Vec<PathBuf>,
    ) -> Result<Self, Error> {
        info!(
            "Initializing background watcher for path: {}, mode: {:?}",
            base_path.display(),
            mode,
        );

        let (watch_tx, watch_rx) = mpsc::channel::<PathBuf>();

        // Clone shared state for the owner thread
        let owner_picker = shared_picker.clone();
        let owner_git_workdir = git_workdir.clone();

        let debouncer = Self::create_debouncer(
            base_path,
            git_workdir,
            shared_picker,
            shared_frecency,
            mode,
            watch_dirs,
            watch_tx,
        )?;
        info!("Background file watcher initialized successfully");

        let stop_signal = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop_signal);

        // The owner thread keeps the debouncer alive and ensures proper
        // cleanup: `Debouncer::stop()` joins its internal thread, then the
        // watcher `Drop` signals its I/O thread to exit.
        let owner_thread = std::thread::Builder::new()
            .name("fff-watcher-owner".into())
            .spawn(move || {
                let mut debouncer = debouncer;
                while !stop_clone.load(Ordering::Acquire) {
                    // Process pending watch requests from the event handler
                    // (new directories that need to be watched).
                    while let Ok(dir) = watch_rx.try_recv() {
                        match debouncer.watch(dir.as_path(), RecursiveMode::NonRecursive) {
                            Ok(()) => {
                                debug!("Added watch for new directory: {}", dir.display());
                            }
                            Err(e) => {
                                warn!("Failed to watch new directory {}: {}", dir.display(), e);
                            }
                        }
                        // Files/dirs created before the watch was set up won't
                        // generate inotify events. Scan and inject them now.
                        scan_new_directory(&dir, &mut debouncer, &owner_picker, &owner_git_workdir);
                    }
                    std::thread::park_timeout(Duration::from_secs(1));
                }
                // Debouncer::stop() joins the debouncer's event thread, then
                // drops the watcher (whose Drop signals the I/O thread).
                debouncer.stop();
                // On Windows the notify crate discards the ReadDirectoryChangesW
                // thread's JoinHandle — we cannot join it. Its Drop signals the
                // thread via semaphore so it exits almost immediately; give the
                // OS a moment to fully reclaim it.
                #[cfg(windows)]
                std::thread::sleep(Duration::from_millis(250));
            })
            .expect("failed to spawn fff-watcher-owner thread");

        Ok(Self {
            stop_signal,
            owner_thread: Some(owner_thread),
        })
    }

    fn create_debouncer(
        base_path: PathBuf,
        git_workdir: Option<PathBuf>,
        shared_picker: SharedPicker,
        shared_frecency: SharedFrecency,
        mode: FFFMode,
        watch_dirs: Vec<PathBuf>,
        watch_tx: mpsc::Sender<PathBuf>,
    ) -> Result<Debouncer, Error> {
        // do not follow symlinks as then notifiers spawns a bunch of events for symlinked
        // files that could be git ignored, we have to property differentiate those and if
        // the file was edited through a
        let config = Config::default().with_follow_symlinks(false);

        let git_workdir_for_handler = git_workdir.clone();
        let mut debouncer = new_debouncer_opt(
            DEBOUNCE_TIMEOUT,
            Some(DEBOUNCE_TIMEOUT / 2), // tick rate for the event span
            {
                move |result: DebounceEventResult| match result {
                    Ok(events) => {
                        // Detect newly created directories and request NonRecursive
                        // watches on them so we see files created inside.
                        // Skip gitignored directories (e.g. node_modules/) to
                        // avoid re-inflating the inotify watch set.
                        let repo = git_workdir_for_handler
                            .as_ref()
                            .and_then(|p| Repository::open(p).ok());
                        for debounced_event in &events {
                            if matches!(
                                debounced_event.event.kind,
                                EventKind::Create(_)
                                    | EventKind::Modify(notify::event::ModifyKind::Name(_))
                            ) {
                                for path in &debounced_event.event.paths {
                                    if path.is_dir()
                                        && !is_git_file(path)
                                        && !is_path_ignored(path, &repo)
                                    {
                                        let _ = watch_tx.send(path.clone());
                                    }
                                }
                            }
                        }

                        handle_debounced_events(
                            events,
                            &git_workdir_for_handler,
                            &shared_picker,
                            &shared_frecency,
                            mode,
                        );
                    }
                    Err(errors) => {
                        error!("File watcher errors: {:?}", errors);
                    }
                }
            },
            // There is an issue with recommended cache implementation on macos
            // it keeps track of all the files added to the watcher which is not a problem
            // for us because any rename to the file will anyway require the removing from the
            // ordedred index and adding it back with the new name
            NoCache::new(),
            config,
        )?;

        // Watch all directories NonRecursively. The watch_dirs are derived from
        // the already-scanned file list so they respect .gitignore at every depth.
        // On Linux (inotify) RecursiveMode::Recursive creates one watch per subdirectory
        // including gitignored ones like node_modules/, which wastes kernel resources.
        // NonRecursive watches only the directories that actually contain indexed files.
        //
        // New directories created at runtime are detected via Create events on the
        // parent and dynamically added by the owner thread via the watch_tx channel.
        debouncer.watch(base_path.as_path(), RecursiveMode::NonRecursive)?;

        for dir in &watch_dirs {
            match debouncer.watch(dir.as_path(), RecursiveMode::NonRecursive) {
                Ok(()) => {}
                Err(e) => {
                    // Non-fatal: directory may have been removed between discovery and watch
                    warn!("Failed to watch directory {}: {}", dir.display(), e);
                }
            }
        }

        // The .git directory is excluded from the file list but we still need
        // to observe changes that affect git status (staging, unstaging,
        // committing, branch switches, merges, etc.).
        watch_git_status_paths(&mut debouncer, git_workdir.as_ref());

        info!(
            "File watcher initialized for {} directories (NonRecursive) under {}",
            watch_dirs.len(),
            base_path.display()
        );

        Ok(debouncer)
    }

    pub fn stop(&mut self) {
        self.stop_signal.store(true, Ordering::Release);
        if let Some(handle) = self.owner_thread.take() {
            handle.thread().unpark();

            if let Err(e) = handle.join() {
                error!("Watcher owner thread panicked: {:?}", e);
            }
        }

        info!("Background file watcher stopped successfully");
    }
}

impl Drop for BackgroundWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[tracing::instrument(name = "fs_events", skip(events, shared_picker, shared_frecency), level = Level::DEBUG)]
fn handle_debounced_events(
    events: Vec<DebouncedEvent>,
    git_workdir: &Option<PathBuf>,
    shared_picker: &SharedPicker,
    shared_frecency: &SharedFrecency,
    mode: FFFMode,
) {
    // this will be called very often, we have to minimiy the lock time for file picker
    let repo = git_workdir.as_ref().and_then(|p| Repository::open(p).ok());
    let mut need_full_rescan = false;
    let mut need_full_git_rescan = false;
    let mut paths_to_remove = Vec::new();
    let mut paths_to_add_or_modify = Vec::new();
    let mut affected_paths_count = 0usize;

    for debounced_event in &events {
        // It is very important to not react to the access errors because we inevitably
        // gonna trigger the sync by our own preview or other unnecessary noise
        if matches!(
            debounced_event.event.kind,
            EventKind::Access(
                AccessKind::Read
                    | AccessKind::Open(_)
                    | AccessKind::Close(AccessMode::Read | AccessMode::Execute)
            )
        ) {
            continue;
        }

        // When macOS FSEvents (or other backends) overflow their event buffer, the kernel
        // drops individual events and emits a Rescan flag telling us to re-scan the subtree.
        // Without handling this, modified source files can be silently missed.
        if debounced_event.event.need_rescan() {
            warn!(
                "Received rescan event for paths {:?}, triggering full rescan",
                debounced_event.event.paths
            );
            need_full_rescan = true;
            break;
        }

        tracing::debug!(event = ?debounced_event.event, "Processing FS event");
        for path in &debounced_event.event.paths {
            if is_ignore_definition_path(path) {
                info!(
                    "Detected change in ignore definition file: {}",
                    path.display()
                );
                need_full_rescan = true;
                break;
            }

            if is_dotgit_change_affecting_status(path, &repo) {
                need_full_git_rescan = true;
            }

            if is_git_file(path) {
                continue;
            }

            // Use a combination of event kind and filesystem state to decide
            // whether a path is an addition/modification or a removal.
            //
            // We cannot rely on `path.exists()` alone because:
            //   - A freshly created file might not be visible yet (race).
            //   - macOS FSEvents uses Modify(Name(Any)) for both rename-in
            //     and rename-out, so we must stat the path to disambiguate.
            //
            // We cannot rely on event kind alone because:
            //   - Remove events are not always emitted (macOS often sends
            //     Modify(Name(Any)) instead of Remove).
            let is_removal = matches!(debounced_event.event.kind, EventKind::Remove(_));

            if is_removal || !path.exists() {
                paths_to_remove.push(path.as_path());
            } else {
                // For additions/modifications, still filter gitignored files.
                if should_include_file(path, &repo) {
                    paths_to_add_or_modify.push(path.as_path());
                }
            }
        }

        affected_paths_count += debounced_event.event.paths.len();
        if affected_paths_count > MAX_PATHS_THRESHOLD {
            warn!(
                "Too many affected paths ({}) in a single batch, triggering full rescan",
                affected_paths_count
            );

            need_full_rescan = true;
            break;
        }

        if need_full_rescan {
            break;
        }
    }

    if need_full_rescan {
        info!(?affected_paths_count, "Triggering full rescan");
        trigger_full_rescan(shared_picker, shared_frecency);
        return;
    }

    // It's important to get the allocated sort
    sort_with_buffer(paths_to_add_or_modify.as_mut_slice(), |a, b| {
        a.as_os_str().cmp(b.as_os_str())
    });
    paths_to_add_or_modify.dedup_by(|a, b| a.as_os_str().eq(b.as_os_str()));

    info!(
        "Event processing summary: {} to remove, {} to add/modify",
        paths_to_remove.len(),
        paths_to_add_or_modify.len()
    );

    // Apply file index updates (add/remove) unconditionally — these must
    // happen even when there is no git repository.
    let files_to_update_git_status =
        if !paths_to_remove.is_empty() || !paths_to_add_or_modify.is_empty() {
            debug!(
                "Applying file index changes: {} to remove, {} to add/modify",
                paths_to_remove.len(),
                paths_to_add_or_modify.len(),
            );

            let apply_changes = |picker: &mut FilePicker| -> Vec<PathBuf> {
                for path in &paths_to_remove {
                    let removed = picker.remove_file_by_path(path);
                    debug!("remove_file_by_path({:?}) -> {}", path, removed);
                }

                let mut files_to_update = Vec::with_capacity(paths_to_add_or_modify.len());
                for path in &paths_to_add_or_modify {
                    let result = picker.on_create_or_modify(path);
                    match result {
                        Some(file) => {
                            debug!(
                                "on_create_or_modify({:?}) -> Some({})",
                                path,
                                file.path_str()
                            );
                            files_to_update.push(PathBuf::from(file.path_str()));
                        }
                        None => {
                            error!("on_create_or_modify({:?}) -> None (file not added!)", path);
                        }
                    }
                }
                info!(
                    "apply_changes complete: {} files to update git status",
                    files_to_update.len()
                );
                files_to_update
            };

            let Ok(mut guard) = shared_picker.write() else {
                error!("Failed to acquire file picker write lock");
                return;
            };
            let Some(ref mut picker) = *guard else {
                error!("File picker not initialized");
                return;
            };
            apply_changes(picker)
        } else {
            debug!("No file index changes to apply");
            Vec::new()
        };

    // AI mode: auto-track frecency for all modified/created files.
    // Uses a 5-minute cooldown per file to prevent score inflation from rapid
    // burst edits (AI agents often edit the same file many times in minutes).
    // This runs after apply_changes so the picker write lock is released.
    if mode.is_ai() && !paths_to_add_or_modify.is_empty() {
        let mut tracked_count = 0usize;
        if let Ok(frecency_guard) = shared_frecency.read()
            && let Some(ref frecency) = *frecency_guard
        {
            for path in &paths_to_add_or_modify {
                // Skip if this file was tracked less than 5 minutes ago
                let should_track = match frecency.seconds_since_last_access(path) {
                    Ok(Some(secs)) => secs >= AI_MODE_COOLDOWN_SECS,
                    Ok(None) => true, // Never tracked before
                    Err(_) => true,   // DB error, track anyway
                };
                if !should_track {
                    continue;
                }

                if let Err(e) = frecency.track_access(path) {
                    error!("Failed to track frecency for {:?}: {:?}", path, e);
                } else {
                    tracked_count += 1;
                }
            }
            if tracked_count > 0 {
                info!("AI mode: tracked frecency for {} files", tracked_count);
            }
        }

        // Update in-memory frecency scores for tracked files
        if tracked_count > 0
            && let Ok(mut picker_guard) = shared_picker.write()
            && let Some(ref mut picker) = *picker_guard
            && let Ok(frecency_guard) = shared_frecency.read()
            && let Some(ref frecency) = *frecency_guard
        {
            for path in &paths_to_add_or_modify {
                let _ = picker.update_single_file_frecency(path, frecency);
            }
        }
    }

    // Git status updates require a repository.
    let Some(repo) = repo.as_ref() else {
        debug!("No git repo available, skipping git status updates");
        return;
    };

    if need_full_git_rescan {
        info!("Triggering full git rescan");

        let result = shared_picker.refresh_git_status(shared_frecency);
        if let Err(e) = result {
            error!("Failed to refresh git status: {:?}", e);
        }
        return;
    }

    if !files_to_update_git_status.is_empty() {
        info!(
            "Fetching git status for {} files",
            files_to_update_git_status.len()
        );

        let status = match GitStatusCache::git_status_for_paths(repo, &files_to_update_git_status) {
            Ok(status) => status,
            Err(e) => {
                tracing::error!(?e, "Failed to query git status");
                return;
            }
        };

        if let Ok(mut guard) = shared_picker.write()
            && let Some(ref mut picker) = *guard
        {
            if let Err(e) = picker.update_git_statuses(status, shared_frecency) {
                error!("Failed to update git statuses: {:?}", e);
            } else {
                info!("Successfully updated git statuses in picker");
            }
        } else {
            error!("Failed to acquire picker lock for git status update");
        }
    }
}

fn trigger_full_rescan(shared_picker: &SharedPicker, shared_frecency: &SharedFrecency) {
    info!("Triggering full filesystem rescan");

    // Note: no need to clear mmaps — they are backed by the kernel page cache
    // and automatically reflect file changes. Old FileItems (and their mmaps)
    // are dropped when the picker rebuilds its file list.

    let Ok(mut guard) = shared_picker.write() else {
        error!("Failed to acquire file picker write lock for full rescan");
        return;
    };
    let Some(ref mut picker) = *guard else {
        error!("File picker not initialized, cannot trigger rescan");
        return;
    };
    if let Err(e) = picker.trigger_rescan(shared_frecency) {
        error!("Failed to trigger full rescan: {:?}", e);
    } else {
        info!("Full filesystem rescan completed successfully");
    }
}

fn should_include_file(path: &Path, repo: &Option<Repository>) -> bool {
    // Directories are not indexed — only regular files (and symlinks to files).
    if path.is_dir() {
        return false;
    }

    match repo.as_ref() {
        Some(repo) => repo.is_path_ignored(path) != Ok(true),
        None => {
            // No git repo — apply basic sanity filters.
            // Hidden directories are skipped by the watcher setup (hidden(true)),
            // but events can still arrive for files in known non-code directories.
            !is_non_code_directory(path)
        }
    }
}

fn is_non_code_directory(path: &Path) -> bool {
    crate::ignore::is_non_code_directory(path)
}

/// After adding a NonRecursive watch on a newly created directory, scan it for
/// files and subdirectories that were created before the watch was set up.
/// This closes the race where `mkdir foo && echo > foo/bar.txt` both happen
/// before the owner thread adds a watch on `foo/`.
fn scan_new_directory(
    dir: &Path,
    debouncer: &mut Debouncer,
    shared_picker: &SharedPicker,
    git_workdir: &Option<PathBuf>,
) {
    let repo = git_workdir.as_ref().and_then(|p| Repository::open(p).ok());
    let mut files_to_add = Vec::new();

    collect_new_entries(dir, &repo, debouncer, &mut files_to_add);

    if files_to_add.is_empty() {
        return;
    }

    let Ok(mut guard) = shared_picker.write() else {
        return;
    };
    let Some(ref mut picker) = *guard else {
        return;
    };

    for path in &files_to_add {
        picker.on_create_or_modify(path);
    }
    info!(
        "Scanned new directory {}: added {} files",
        dir.display(),
        files_to_add.len(),
    );
}

fn collect_new_entries(
    dir: &Path,
    repo: &Option<Repository>,
    debouncer: &mut Debouncer,
    files: &mut Vec<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_dir() {
            if !is_git_file(&path) && !is_path_ignored(&path, repo) {
                let _ = debouncer.watch(&path, RecursiveMode::NonRecursive);
                collect_new_entries(&path, repo, debouncer, files);
            }
        } else if file_type.is_file() && should_include_file(&path, repo) {
            files.push(path);
        }
    }
}

#[inline]
fn is_path_ignored(path: &Path, repo: &Option<Repository>) -> bool {
    match repo.as_ref() {
        Some(repo) => repo.is_path_ignored(path) == Ok(true),
        None => is_non_code_directory(path),
    }
}

#[inline]
fn is_git_file(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == ".git")
}

pub fn is_dotgit_change_affecting_status(changed: &Path, repo: &Option<Repository>) -> bool {
    let Some(repo) = repo.as_ref() else {
        return false;
    };

    let git_dir = repo.path();

    if let Ok(rel) = changed.strip_prefix(git_dir) {
        if rel.starts_with("objects") || rel.starts_with("logs") || rel.starts_with("hooks") {
            return false;
        }
        if rel == Path::new("index") || rel == Path::new("index.lock") {
            return true;
        }
        if rel == Path::new("HEAD") {
            return true;
        }
        if rel.starts_with("refs") || rel == Path::new("packed-refs") {
            return true;
        }
        if rel == Path::new("info/exclude") || rel == Path::new("info/sparse-checkout") {
            return true;
        }

        if let Some(fname) = rel.file_name().and_then(|f| f.to_str())
            && matches!(fname, "MERGE_HEAD" | "CHERRY_PICK_HEAD" | "REVERT_HEAD")
        {
            return true;
        }
    }

    false
}

fn is_ignore_definition_path(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|f| f.to_str()),
        Some(".ignore") | Some(".gitignore")
    )
}

fn watch_git_status_paths(debouncer: &mut Debouncer, git_workdir: Option<&PathBuf>) {
    let Some(workdir) = git_workdir else {
        return;
    };

    let git_dir = workdir.join(".git");
    if !git_dir.is_dir() {
        return;
    }

    // Watch .git/ non-recursively to catch top-level files:
    // index, index.lock, HEAD, packed-refs, MERGE_HEAD, CHERRY_PICK_HEAD, REVERT_HEAD
    if let Err(e) = debouncer.watch(&git_dir, RecursiveMode::NonRecursive) {
        warn!("Failed to watch .git directory: {}", e);
        return;
    }

    // Watch refs/ recursively to catch branch/tag changes
    let refs_dir = git_dir.join("refs");
    if refs_dir.is_dir()
        && let Err(e) = debouncer.watch(&refs_dir, RecursiveMode::Recursive)
    {
        warn!("Failed to watch .git/refs: {}", e);
    }

    // Watch info/ non-recursively for exclude and sparse-checkout
    let info_dir = git_dir.join("info");
    if info_dir.is_dir()
        && let Err(e) = debouncer.watch(&info_dir, RecursiveMode::NonRecursive)
    {
        warn!("Failed to watch .git/info: {}", e);
    }
}
