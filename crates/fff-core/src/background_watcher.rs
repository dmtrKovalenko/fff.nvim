use crate::constants::MAX_OVERFLOW_FILES;
use crate::error::Error;
use crate::file_picker::FFFMode;
use crate::git_status_worker::GitStatusWorker;
use crate::shared::{SharedFilePicker, SharedFrecency};
use crate::sort_buffer::sort_with_buffer;
use git2::Repository;
use notify::event::{AccessKind, AccessMode};
use notify::{Config, EventKind, EventKindMask, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, DebouncedEvent, NoCache, new_debouncer_opt};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;
use tracing::{Level, debug, error, info, warn};

type Debouncer = notify_debouncer_full::Debouncer<notify::RecommendedWatcher, NoCache>;

/// Owns the file-system watcher and guarantees that all background threads
/// are fully joined before `stop()` / `Drop` returns.
pub struct BackgroundWatcher {
    debouncer: Arc<Mutex<Option<Debouncer>>>,
    watch_tx: Option<mpsc::Sender<PathBuf>>,
    owner_thread: Option<std::thread::JoinHandle<()>>,
}

const DEBOUNCE_TIMEOUT: Duration = Duration::from_millis(50);
/// On macOS, each `watch()` call creates a separate FSEventStream. When the
/// number of directories exceeds this threshold we fall back to a single
/// recursive watch to avoid exhausting the per-process stream limit.
const MAX_MACOS_NONRECURSIVE_WATCHES: usize = 4096;
/// Minimum seconds between frecency tracks of the same file in AI mode.
/// Prevents score inflation from rapid burst edits by AI agents.
const AI_MODE_COOLDOWN_SECS: u64 = 5 * 60;

impl BackgroundWatcher {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_path: PathBuf,
        git_workdir: Option<PathBuf>,
        shared_picker: SharedFilePicker,
        shared_frecency: SharedFrecency,
        mode: FFFMode,
        enable_fs_root_scanning: bool,
        enable_home_dir_scanning: bool,
        git_status_worker: Arc<GitStatusWorker>,
        trace_span: tracing::Span,
    ) -> Result<Self, Error> {
        info!(
            "Initializing background watcher for path: {}, mode: {:?}",
            base_path.display(),
            mode,
        );

        // by default we do not want to allow users to search their FS root, this is very error prone
        // though some consumers would specifically allow that e.g. unikernels, windows disc
        // partition or sub file systems. By default - fail, unless user permits
        let is_fs_root = base_path.parent().is_none();
        // use rust's path api for maximum reliability of the comparison
        let is_home_dir = Some(&base_path) == dirs::home_dir().as_ref();

        if (is_fs_root && !enable_fs_root_scanning) || (is_home_dir && !enable_home_dir_scanning) {
            return Err(Error::FilesystemRoot(base_path));
        }

        // macOS: always use a single recursive FSEvent stream.
        // Per-dir NonRecursive watches create one FSEvent stream per dir.
        // The per-process FSEvent cap is lower than expected in practice
        // (4096 per process, but FFF usually is running within code editors),
        // and each failed `watch()` after the cap blocks ~40 ms on kernel retry.
        // Yes we pay for filtering events on handler phase but it is usable
        //
        // Windows doesn't seem to have a hard cap, but in practice non recursive watching
        // does a way worse job and often looses events which is not an option for us.
        //
        // Linux keeps the per-dir NonRecursive strategy: inotify has no
        // kernel-level watcher recursion, so we have to manually watch every single interested
        // directory for watch events which is in practice stable and fast if system has enough
        // spare watcher (configurable by the user, usually 100k - 1m)
        let use_recursive = cfg!(any(target_os = "macos", target_os = "windows"));

        let (watch_tx, watch_rx) = mpsc::channel::<PathBuf>();
        let watch_tx_for_debouncer = watch_tx.clone();

        let owner_weak_picker = shared_picker.weaken();
        let owner_git_workdir = git_workdir.clone();
        let owner_git_worker = Arc::clone(&git_status_worker);

        let debouncer = Self::create_debouncer(
            base_path,
            git_workdir,
            shared_picker,
            shared_frecency,
            mode,
            use_recursive,
            watch_tx_for_debouncer,
            git_status_worker,
        )?;

        info!("Background file watcher initialized successfully");

        // debouncer is shared with the owner thread, once it's dropped the thread is closed
        let debouncer = Arc::new(Mutex::new(Some(debouncer)));
        // Only the Linux per-dir-watch branch needs this clone; on other
        // platforms the owner thread never touches the debouncer.
        #[cfg(target_os = "linux")]
        let owner_debouncer = Arc::clone(&debouncer);

        let owner_span = trace_span.clone();
        let owner_thread = std::thread::Builder::new()
            .name("fff-watcher-own".into())
            .spawn(move || {
                let _g = owner_span.enter();
                while let Ok(dir) = watch_rx.recv() {
                    // if the picker is dropped we do need to exit the loop
                    let Some(strong_picker) = owner_weak_picker.upgrade() else {
                        break;
                    };

                    // Only inotify (Linux) has no kernel-level recursion, so
                    // it's the only platform that needs a per-subdir watch to
                    // be registered at runtime. macOS FSEvents and Windows
                    // ReadDirectoryChangesW are already watching recursively
                    // from the base path (see `create_debouncer`), and
                    // registering a second overlapping stream there produces
                    // duplicate/out-of-order events.
                    #[cfg(target_os = "linux")]
                    {
                        // Register the new directory with the debouncer, then
                        // drop the mutex BEFORE doing picker-side work — see
                        // the comment on `BackgroundWatcher::stop` for the
                        // lock-ordering rationale.
                        let mut guard = owner_debouncer.lock();
                        let Some(debouncer) = guard.as_mut() else {
                            break;
                        };

                        if let Err(e) = debouncer.watch(&dir, RecursiveMode::NonRecursive) {
                            warn!(
                                ?e,
                                dir = %dir.display(),
                                "Failed to init watcher for new directory"
                            );
                        }
                    }

                    track_files_from_new_directories(
                        &dir,
                        &strong_picker,
                        &owner_git_workdir,
                        &owner_git_worker,
                    );

                    // Transient strong ref drops here, back
                    // to weak-only before the next `recv()`.
                }

                tracing::info!("Background watcher is stopped");
            })
            .expect("failed to spawn fff-watcher-owner thread");

        Ok(Self {
            debouncer,
            watch_tx: Some(watch_tx),
            owner_thread: Some(owner_thread),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn create_debouncer(
        base_path: PathBuf,
        git_workdir: Option<PathBuf>,
        shared_picker: SharedFilePicker,
        shared_frecency: SharedFrecency,
        mode: FFFMode,
        use_recursive: bool,
        watch_tx: mpsc::Sender<PathBuf>,
        git_status_worker: Arc<GitStatusWorker>,
    ) -> Result<Debouncer, Error> {
        let config = Config::default()
            // do not follow symlinks as then notifiers spawns a bunch of events for symlinked
            // files that could be git ignored, we have to property differentiate those and if
            // the file was edited through a
            .with_follow_symlinks(false)
            // only the actual modification events, ignore the open syscals that we can generate by
            // our own grep calls and preview window rendering
            .with_event_kinds(EventKindMask::CORE);

        let git_workdir_for_handler = git_workdir.clone();
        let base_path_for_handler = base_path.clone();
        let shared_picker_for_watching = shared_picker.clone();
        let file_picker = shared_picker.weaken();
        let mut debouncer = new_debouncer_opt(
            DEBOUNCE_TIMEOUT,
            Some(DEBOUNCE_TIMEOUT / 2), // tick rate for the event span
            {
                move |result: DebounceEventResult| match result {
                    Ok(events) => {
                        let Some(file_picker) = file_picker.upgrade() else {
                            return;
                        };

                        let new_dirs = handle_debounced_events(
                            mode,
                            events,
                            &base_path_for_handler,
                            &git_workdir_for_handler,
                            &file_picker,
                            &shared_frecency,
                            &git_status_worker,
                        );

                        // every new directory created has to be reflected in the picker state
                        for dir in new_dirs {
                            if let Err(e) = watch_tx.send(dir) {
                                error!(?e, "Failed to send directory update error");
                            }
                        }
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

        if use_recursive {
            // if the platform supports native watcher recursion
            debouncer.watch(base_path.as_path(), RecursiveMode::Recursive)?;
            info!(
                "File watcher initialized with single recursive watch on {} \
                 (exceeded threshold of {})",
                base_path.display(),
                MAX_MACOS_NONRECURSIVE_WATCHES,
            );
        } else {
            debouncer.watch(base_path.as_path(), RecursiveMode::NonRecursive)?;

            const MAX_CONSECUTIVE_WATCH_FAILURES: usize = 16;

            let mut watched = 0usize;
            let mut consecutive_failures = 0usize;

            // `inotify` is fast-fail: on ENOSPC it returns
            // immediately, no kernel retry loop, so holding this lock is free
            if let Some(guard) = shared_picker_for_watching.read().ok()
                && let Some(picker) = guard.as_ref()
            {
                use std::ops::ControlFlow;
                picker.for_each_dir(|dir| {
                    match debouncer.watch(dir, RecursiveMode::NonRecursive) {
                        Ok(()) => {
                            watched += 1;
                            consecutive_failures = 0;
                            ControlFlow::Continue(())
                        }
                        Err(e) => {
                            consecutive_failures += 1;
                            if consecutive_failures <= 4 {
                                warn!("Failed to watch directory {}: {}", dir.display(), e);
                            }

                            if consecutive_failures >= MAX_CONSECUTIVE_WATCH_FAILURES {
                                warn!(
                                    consecutive_failures,
                                    watched,
                                    "Giving up setting file watcher for all the directories. Check if your system has enough fs watchers limit."
                                );

                                ControlFlow::Break(())
                            } else {
                                ControlFlow::Continue(())
                            }
                        }
                    }
                });
            }

            tracing::info!(
                ?watched,
                path = ?base_path.display(),
                "File watcher initialized"
            );
        }

        // The .git directory is excluded from the file list but we still need
        // to observe changes that affect git status (staging, unstaging,
        // committing, branch switches, merges, etc)
        watch_git_status_paths(&mut debouncer, git_workdir.as_ref());

        Ok(debouncer)
    }

    /// Signals the background watcher threads to shut down, doesn't guarantee to deallocate immediately
    pub fn stop(&mut self) {
        self.watch_tx.take();
        if let Some(debouncer) = self.debouncer.lock().take() {
            debouncer.stop_nonblocking();
        }

        self.owner_thread.take();

        info!("Background file watcher stop signaled");
    }

    pub(crate) fn request_watch_dir(&self, dir: PathBuf) -> bool {
        match self.watch_tx.as_ref() {
            Some(tx) => tx.send(dir).is_ok(),
            None => false,
        }
    }
}

impl Drop for BackgroundWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[tracing::instrument(name = "fs_events", skip(events, shared_picker, shared_frecency, git_status_worker), level = Level::DEBUG)]
fn handle_debounced_events(
    mode: FFFMode,
    events: Vec<DebouncedEvent>,
    base_path: &Path,
    git_workdir: &Option<PathBuf>,
    shared_picker: &SharedFilePicker,
    shared_frecency: &SharedFrecency,
    git_status_worker: &Arc<GitStatusWorker>,
) -> Vec<PathBuf> {
    // this will be called very often, we have to minimiy the lock time for file picker
    let repo = git_workdir.as_ref().and_then(|p| Repository::open(p).ok());
    // Prefer the walker's own ignore rules (zlob); grab a cheap Arc clone once
    // per batch so we don't hold the picker lock during filtering.
    let walker_rules = shared_picker
        .read()
        .ok()
        .and_then(|g| g.as_ref().and_then(|p| p.ignore_rules()));
    let filter = IgnoreFilter::new(base_path, walker_rules, repo.as_ref());
    let mut need_full_rescan = false;
    let mut need_full_git_rescan = false;
    let mut paths_to_remove = Vec::new();
    let mut dirs_to_remove: Vec<PathBuf> = Vec::new();
    let mut paths_to_add_or_modify = Vec::new();
    let mut new_dirs_to_watch = Vec::new();
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
        // drops individual events and emits a rescan flag telling us to re-scan the subtree
        if debounced_event.event.need_rescan() {
            if debounced_event.event.paths.len() < 16 // this should be usually one event
                && debounced_event
                    .paths
                    .iter()
                    // but we are smart enough and not falling into the paths
                    .all(|p| should_include_file(p, &filter))
            {
                break;
            }

            warn!(
                "Received rescan event for paths {:?}, triggering full rescan",
                debounced_event.event.paths
            );
            need_full_rescan = true;
            break;
        }

        tracing::debug!(event = ?debounced_event.event, "Processing FS event");
        for path in &debounced_event.event.paths {
            if matches!(
                path.file_name().and_then(|f| f.to_str()),
                Some(".ignore") | Some(".gitignore")
            ) {
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

            // Directory-level remove: both fsevents and inotify delivers a single
            // `Remove(Folder)` event for a whole directory tree (e.g.
            // after `git reset --hard` wipes a dir full of staged-but-
            // uncommitted files).
            let is_folder_removal = matches!(
                debounced_event.event.kind,
                EventKind::Remove(notify::event::RemoveKind::Folder)
            );

            if is_folder_removal {
                dirs_to_remove.push(path.to_path_buf());
            } else if is_removal || !path.exists() {
                paths_to_remove.push(path.as_path());
            } else if path.is_dir() {
                if !is_path_ignored(path, &filter) {
                    new_dirs_to_watch.push(path.to_path_buf());
                }
            } else {
                // For additions/modifications, still filter gitignored files.
                if should_include_file(path, &filter) {
                    paths_to_add_or_modify.push(path.as_path());
                }
            }
        }

        affected_paths_count += debounced_event.event.paths.len();
        if affected_paths_count > MAX_OVERFLOW_FILES {
            warn!(
                ?affected_paths_count,
                max = MAX_OVERFLOW_FILES,
                "Too many affected paths in a single batch, triggering full rescan",
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
        if let Err(e) = shared_picker.trigger_full_rescan_async(shared_frecency) {
            error!("Failed to trigger full rescan: {:?}", e);
        }
        return Vec::new();
    }

    // It's important to get the allocated sort
    sort_with_buffer(paths_to_add_or_modify.as_mut_slice(), |a, b| {
        a.as_os_str().cmp(b.as_os_str())
    });
    paths_to_add_or_modify.dedup_by(|a, b| a.as_os_str().eq(b.as_os_str()));

    info!(
        "Event processing summary: {} to remove, {} dirs to remove, {} to add/modify, {} new dirs",
        paths_to_remove.len(),
        dirs_to_remove.len(),
        paths_to_add_or_modify.len(),
        new_dirs_to_watch.len()
    );

    if paths_to_remove.is_empty()
        && dirs_to_remove.is_empty()
        && paths_to_add_or_modify.is_empty()
        && !need_full_git_rescan
    {
        debug!("No file index changes to apply");
        return new_dirs_to_watch;
    }

    let mut files_to_update_git_status = Vec::new();
    let mut need_full_rescan = false;
    let mut overflow_count = 0;

    if !paths_to_remove.is_empty()
        || !dirs_to_remove.is_empty()
        || !paths_to_add_or_modify.is_empty()
    {
        debug!(
            "Applying file index changes: {} to remove, {} dirs to remove, {} to add/modify",
            paths_to_remove.len(),
            dirs_to_remove.len(),
            paths_to_add_or_modify.len(),
        );

        let Ok(mut guard) = shared_picker.write() else {
            error!("Failed to acquire file picker write lock");
            return new_dirs_to_watch;
        };
        let Some(ref mut picker) = *guard else {
            error!("File picker not initialized");
            return new_dirs_to_watch;
        };

        for dir in &dirs_to_remove {
            let count = picker.remove_all_files_in_dir(dir);
            debug!("remove_all_files_in_dir({:?}) -> {} files", dir, count);
        }

        for path in &paths_to_remove {
            let removed = picker.remove_file_by_path(path);
            debug!("remove_file_by_path({:?}) -> {}", path, removed);
        }

        files_to_update_git_status.reserve(paths_to_add_or_modify.len());
        for path in &paths_to_add_or_modify {
            if picker.handle_create_or_modify(path).is_some() {
                files_to_update_git_status.push(path.to_path_buf());
            } else {
                need_full_rescan = true;
            }
        }

        overflow_count = picker.get_overflow_files().len();
    }

    info!(
        files_updated = files_to_update_git_status.len(),
        overflow_count, "File index changes applied",
    );
    if need_full_rescan || overflow_count > MAX_OVERFLOW_FILES {
        info!("Watcher faced limit of index overflow. Triggering rescan");
        if let Err(e) = shared_picker.trigger_full_rescan_async(shared_frecency) {
            error!("Failed to trigger full rescan: {:?}", e);
        }
    }

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

    // do not try to update the paths if we anyway going to rescan everything from scratch
    // no repo => no consumer thread, so don't accumulate paths nobody will drain
    if !need_full_rescan && repo.is_some() {
        if need_full_git_rescan {
            // A full git rescan re-reads every tracked path (including ones that just
            // went clean after a commit), so it already subsumes the per-path update.
            git_status_worker.request_full_rescan();
        } else if !files_to_update_git_status.is_empty() {
            git_status_worker.enqueue_paths(files_to_update_git_status);
        }
    }

    new_dirs_to_watch
}

/// After registering a watch on a newly created directory, list its
/// immediate children and add any files to the picker.
fn track_files_from_new_directories(
    dir: &Path,
    shared_picker: &SharedFilePicker,
    git_workdir: &Option<PathBuf>,
    git_status_worker: &Arc<GitStatusWorker>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    let repo = git_workdir.as_ref().and_then(|p| Repository::open(p).ok());
    // Prefer the walker's ignore rules; read base_path + rules from the picker.
    let (base_path, walker_rules) = match shared_picker.read().ok().and_then(|g| {
        g.as_ref()
            .map(|p| (p.base_path().to_path_buf(), p.ignore_rules()))
    }) {
        Some(pair) => pair,
        None => return,
    };

    let filter = IgnoreFilter::new(&base_path, walker_rules, repo.as_ref());
    let mut files_to_add = Vec::new();

    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|ft| ft.is_file()) {
            let path = entry.path();
            if should_include_file(&path, &filter) {
                files_to_add.push(path);
            }
        }
    }

    if files_to_add.is_empty() {
        return;
    }

    let added = files_to_add.len();

    {
        let Ok(mut guard) = shared_picker.write() else {
            return;
        };

        let Some(ref mut picker) = *guard else {
            return;
        };

        for path in &files_to_add {
            picker.handle_create_or_modify(path);
        }
    }

    if repo.is_some() {
        git_status_worker.enqueue_paths(files_to_add);
    }

    debug!(
        "Injected {} existing files from new directory {}",
        added,
        dir.display(),
    );
}

fn should_include_file(path: &Path, filter: &IgnoreFilter) -> bool {
    // Directories are not indexed — only regular files (and symlinks to files).
    if path.is_dir() {
        return false;
    }
    !filter.is_ignored(path)
}

#[inline]
fn is_path_ignored(path: &Path, filter: &IgnoreFilter) -> bool {
    filter.is_ignored(path)
}

struct IgnoreFilter<'a> {
    base_path: &'a Path,
    /// Reusable ignore rules from the last walk (zlob backend only).
    rules: Option<Arc<crate::walk::WalkIgnoreRules>>,
    /// libgit2 repo, consulted only when `rules` is `None`. Borrowed from the
    /// caller's repo (also used for git-status queries) to avoid re-opening.
    repo: Option<&'a Repository>,
}

impl<'a> IgnoreFilter<'a> {
    fn new(
        base_path: &'a Path,
        rules: Option<Arc<crate::walk::WalkIgnoreRules>>,
        repo: Option<&'a Repository>,
    ) -> Self {
        Self {
            base_path,
            rules,
            repo,
        }
    }

    /// Whether `path` (absolute) is ignored.
    fn is_ignored(&self, path: &Path) -> bool {
        if let Some(rules) = self.rules.as_ref() {
            let Ok(rel) = path.strip_prefix(self.base_path) else {
                return false;
            };
            // `IgnoreRules::is_ignored` enumerates every ancestor .gitignore
            // layer internally, so a leaf under an ignored directory (rule
            // `build/`, path `build/out.rs`) is caught in one call.
            return rules.is_ignored(rel);
        }
        match self.repo {
            Some(repo) => repo.is_path_ignored(path) == Ok(true),
            // No repo and no rules: fall back to the non-code-dir heuristic.
            None => crate::ignore::is_non_code_directory(path),
        }
    }
}

#[inline]
pub(crate) fn is_git_file(path: &Path) -> bool {
    // it could be in submodule
    path.components()
        .any(|component| component.as_os_str() == ".git")
}

fn is_dotgit_change_affecting_status(changed: &Path, repo: &Option<Repository>) -> bool {
    let Some(repo) = repo.as_ref() else {
        return false;
    };

    let git_dir = repo.path();

    if let Ok(path_in_git_dir) = changed.strip_prefix(git_dir) {
        // Only react to changes that rewrite the worktree state: commits,
        // staging, checkouts, merges, conflict resolution. Ref-only updates
        // under refs/ (fetch, push, tag writes, pack-refs) do not change
        // which files are modified/untracked, so we deliberately skip them
        // watching refs/ recursively would cost one inotify watch per ref
        // namespace on repos with many branches/remotes.
        if path_in_git_dir == Path::new("index") || path_in_git_dir == Path::new("index.lock") {
            return true;
        }

        if path_in_git_dir == Path::new("HEAD") {
            return true;
        }

        // some of the git ops are not involving nethier index nor HEAD change, or sometimes
        // index updates can arrive too late after the change - that's why we track the log
        // the actual user action, once user
        if path_in_git_dir == Path::new("logs/HEAD") {
            return true;
        }

        if let Some(fname) = path_in_git_dir.file_name().and_then(|f| f.to_str())
            && matches!(fname, "MERGE_HEAD" | "CHERRY_PICK_HEAD" | "REVERT_HEAD")
        {
            return true;
        }
    }

    false
}

fn watch_git_status_paths(debouncer: &mut Debouncer, git_workdir: Option<&PathBuf>) {
    let Some(workdir) = git_workdir else {
        return;
    };

    let git_dir = workdir.join(".git");
    if !git_dir.is_dir() {
        return;
    }

    // We have tried to be smart about the internal git state but
    // it appeared more harmful that it's worth it, so we just watch
    // for the most obvious paths like HEAD, MERGE_HEAD, index.lock
    if let Err(e) = debouncer.watch(&git_dir, RecursiveMode::NonRecursive) {
        warn!("Failed to watch .git directory: {}", e);
    }

    // `.git` above is non-recursive, so on Linux (per-dir inotify watches)
    // events for `logs/HEAD` — the commit-finished signal used by
    // `is_dotgit_change_affecting_status` — would never be delivered without
    // watching `.git/logs` itself. On macOS/Windows the recursive base watch
    // already covers it; an extra watch is harmless there.
    let logs_dir = git_dir.join("logs");
    if logs_dir.is_dir()
        && let Err(e) = debouncer.watch(&logs_dir, RecursiveMode::NonRecursive)
    {
        warn!("Failed to watch .git/logs directory: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotgit_status_filter_matches_worktree_state_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(tmp.path()).unwrap();
        let git_dir = repo.path().to_path_buf();
        let repo = Some(repo);

        let affecting = ["index", "index.lock", "HEAD", "logs/HEAD", "MERGE_HEAD"];
        for p in affecting {
            assert!(
                is_dotgit_change_affecting_status(&git_dir.join(p), &repo),
                "{p} must trigger a git status rescan"
            );
        }

        // Ref-only updates (fetch/push/tags) and commit scratch files must not.
        let non_affecting = [
            "refs/heads/main",
            "refs/heads/main.lock",
            "logs/refs/remotes/origin/main",
            "COMMIT_EDITMSG",
            "packed-refs",
        ];
        for p in non_affecting {
            assert!(
                !is_dotgit_change_affecting_status(&git_dir.join(p), &repo),
                "{p} must NOT trigger a git status rescan"
            );
        }

        // Worktree paths outside .git never match.
        assert!(!is_dotgit_change_affecting_status(
            &tmp.path().join("src/main.rs"),
            &repo
        ));
        assert!(!is_dotgit_change_affecting_status(
            &git_dir.join("index"),
            &None
        ));
    }
}
