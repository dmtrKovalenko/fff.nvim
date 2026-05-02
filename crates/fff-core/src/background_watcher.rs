use crate::error::Error;
use crate::file_picker::{FFFMode, FilePicker};
use crate::git::GitStatusCache;
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
const MAX_PATHS_THRESHOLD: usize = 1024;
/// On macOS, each `watch()` call creates a separate FSEventStream. When the
/// number of directories exceeds this threshold we fall back to a single
/// recursive watch to avoid exhausting the per-process stream limit.
const MAX_MACOS_NONRECURSIVE_WATCHES: usize = 4096;
/// Minimum seconds between frecency tracks of the same file in AI mode.
/// Prevents score inflation from rapid burst edits by AI agents.
const AI_MODE_COOLDOWN_SECS: u64 = 5 * 60;
const MAX_OVERFLOW_FILES: usize = 1024;

impl BackgroundWatcher {
    pub fn new(
        base_path: PathBuf,
        git_workdir: Option<PathBuf>,
        shared_picker: SharedFilePicker,
        shared_frecency: SharedFrecency,
        mode: FFFMode,
    ) -> Result<Self, Error> {
        info!(
            "Initializing background watcher for path: {}, mode: {:?}",
            base_path.display(),
            mode,
        );

        // Refuse to watch the filesystem root or the user's home directory.
        // These are prone to high-volume event churn (editor temp files,
        // browser caches, log rotations) which inflates the overflow arena
        // and, on macOS, can exhaust the per-process FSEvents stream limit.
        if base_path.parent().is_none()
            || Some(base_path.as_os_str()) == dirs::home_dir().as_ref().map(|p| p.as_os_str())
        {
            return Err(Error::FilesystemRoot(base_path));
        }

        // macOS: always use a single recursive FSEvent stream.
        //
        // Per-dir NonRecursive watches create one FSEvent stream per dir.
        // The per-process FSEvent cap is lower than expected in practice
        // (4096 per process, but FFF usually is running within code editors),
        // and each failed `watch()` after the cap blocks ~40 ms on kernel retry.
        // Yes we pay for filtering events on handler phase but it is usable
        //
        // macOS and Windows use a single recursive watch. FSEvents and
        // ReadDirectoryChangesW both support true kernel-level recursion
        // on one handle — per-dir NonRecursive watches burn streams/handles
        // for no benefit and, on Windows, have been observed to silently
        // drop Modify events for nested paths.
        //
        // Linux keeps the per-dir NonRecursive strategy: inotify has no
        // kernel-level recursion, so Recursive here would still register
        // one watch per subdir but without the ignored-dir filtering we
        // get by iterating `picker.for_each_dir` ourselves.
        let use_recursive = cfg!(any(target_os = "macos", target_os = "windows"));

        let (watch_tx, watch_rx) = mpsc::channel::<PathBuf>();
        let watch_tx_for_debouncer = watch_tx.clone();

        let owner_weak_picker = shared_picker.weaken();
        let owner_frecency = shared_frecency.clone();
        let owner_git_workdir = git_workdir.clone();

        let debouncer = Self::create_debouncer(
            base_path,
            git_workdir,
            shared_picker,
            shared_frecency,
            mode,
            use_recursive,
            watch_tx_for_debouncer,
        )?;

        info!("Background file watcher initialized successfully");

        // debouncer is shared with the owner thread, once it's dropped the thread is closed
        let debouncer = Arc::new(Mutex::new(Some(debouncer)));
        // Only the Linux per-dir-watch branch needs this clone; on other
        // platforms the owner thread never touches the debouncer.
        #[cfg(target_os = "linux")]
        let owner_debouncer = Arc::clone(&debouncer);

        let owner_thread = std::thread::Builder::new()
            .name("fff-watcher-own".into())
            .spawn(move || {
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
                        &owner_frecency,
                        &owner_git_workdir,
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

    fn create_debouncer(
        base_path: PathBuf,
        git_workdir: Option<PathBuf>,
        shared_picker: SharedFilePicker,
        shared_frecency: SharedFrecency,
        mode: FFFMode,
        use_recursive: bool,
        watch_tx: mpsc::Sender<PathBuf>,
    ) -> Result<Debouncer, Error> {
        let config = Config::default()
            // do not follow symlinks as then notifiers spawns a bunch of events for symlinked
            // files that could be git ignored, we have to property differentiate those and if
            // the file was edited through a
            .with_follow_symlinks(false)
            // only the actual modification events, ignore the open syscals that we can generate by
            // our own grep calls and preview window rendering
            .with_event_kinds(EventKindMask::CORE);

        // `use_recursive` was decided by the caller from a cheap size hint,
        // so the event-handler closure can capture it directly.
        //
        // The closure lives on the debouncer's internal event thread
        // for as long as the debouncer exists — i.e. the full
        // lifetime of `BackgroundWatcher`. Capturing a strong
        // `SharedFilePicker` here would re-introduce the Arc cycle
        // we just broke with `owner_picker`'s `downgrade()` above.
        // Capture a weak handle instead and upgrade per-batch.
        let git_workdir_for_handler = git_workdir.clone();
        let shared_picker_for_watching = shared_picker.clone();
        let event_picker = shared_picker.weaken();
        let mut debouncer = new_debouncer_opt(
            DEBOUNCE_TIMEOUT,
            Some(DEBOUNCE_TIMEOUT / 2), // tick rate for the event span
            {
                move |result: DebounceEventResult| match result {
                    Ok(events) => {
                        // Upgrade just long enough to drive one
                        // debounced batch. Failure means every
                        // external `SharedFilePicker` has already
                        // dropped and teardown is already underway.
                        let Some(strong_picker) = event_picker.upgrade() else {
                            return;
                        };

                        let new_dirs = handle_debounced_events(
                            events,
                            &git_workdir_for_handler,
                            &strong_picker,
                            &shared_frecency,
                            mode,
                        );

                        // every new directory creates had to be reflected in the picker state
                        for dir in new_dirs {
                            if let Err(e) = watch_tx.send(dir) {
                                warn!(?e, "Failed to send directory update error");
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

        // Watching strategy:
        //
        // For small-to-medium repos we watch each indexed directory individually
        // (NonRecursive). This avoids receiving events for gitignored paths like
        // node_modules/ and keeps the event volume low.
        //
        // On macOS, each `watch()` call creates a separate FSEventStream. Large
        // repos (e.g. Chromium with 487K+ files) can have tens of thousands of
        // directories, which exhausts the per-process FSEvents stream limit and
        // causes "unable to start FSEvent stream" errors. When the directory
        // count exceeds the threshold we fall back to a single Recursive watch
        // on the base path. FSEvents handles this efficiently with one kernel
        // stream for the entire subtree. Gitignored paths are already filtered
        // in the event handler via `should_include_file()`.
        //
        // On Linux (inotify), RecursiveMode::Recursive creates one kernel watch
        // per subdirectory *including* gitignored ones, wasting file descriptors.
        // The per-directory NonRecursive approach is always used on Linux.
        //
        // New directories created at runtime are detected via Create events on
        // the parent and dynamically added by the owner thread via watch_tx.

        if use_recursive {
            debouncer.watch(base_path.as_path(), RecursiveMode::Recursive)?;
            info!(
                "File watcher initialized with single recursive watch on {} \
                 (exceeded threshold of {})",
                base_path.display(),
                MAX_MACOS_NONRECURSIVE_WATCHES,
            );
        } else {
            debouncer.watch(base_path.as_path(), RecursiveMode::NonRecursive)?;

            // Stream watch-dir registration directly under the picker
            // read lock. Only Linux (inotify) reaches this branch —
            // macOS always takes the recursive path above. `inotify`'s
            // `inotify_add_watch()` is fast-fail: on ENOSPC it returns
            // immediately, no kernel retry loop, so holding the read
            // lock across the stream is O(ms) even for large repos.
            //
            // Abort the loop after a run of failures. Once ENOSPC hits,
            // further calls won't succeed until the user raises
            // `fs.inotify.max_user_watches`, so there's no value in
            // continuing.
            const MAX_CONSECUTIVE_WATCH_FAILURES: usize = 16;

            let mut watched = 0usize;
            let mut consecutive_failures = 0usize;
            let mut aborted_early = false;

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
                                    "Aborting NonRecursive watch loop — per-process \
                                 watch cap exhausted, further dirs would just burn \
                                 kernel time for no coverage"
                                );
                                aborted_early = true;
                                ControlFlow::Break(())
                            } else {
                                ControlFlow::Continue(())
                            }
                        }
                    }
                });
            }

            info!(
                "File watcher initialized for {} directories (NonRecursive) under {} (aborted_early={})",
                watched,
                base_path.display(),
                aborted_early,
            );
        }

        // The .git directory is excluded from the file list but we still need
        // to observe changes that affect git status (staging, unstaging,
        // committing, branch switches, merges, etc.).
        // When using recursive mode the base watch already covers .git/,
        // but these targeted watches are cheap (at most 3 extra streams)
        // and ensure we catch status changes even if the recursive backend
        // coalesces or delays .git events.
        watch_git_status_paths(&mut debouncer, git_workdir.as_ref());

        Ok(debouncer)
    }

    /// Signal the watcher to shut down without blocking on its worker
    /// threads. Safe to call from any context, including while holding
    /// the [`SharedFilePicker`] write lock.
    ///
    /// Both the debouncer's internal event loop and our owner thread
    /// may call `SharedFilePicker::write()` inside their handlers. A
    /// blocking join here would deadlock against a caller that already
    /// holds that lock (e.g. `stop_background_monitor` under a
    /// `shared_picker.write()` guard). Instead we:
    ///
    ///   * drop the `watch_tx` Sender — the owner thread's
    ///     `watch_rx.recv()` returns `Err` and the thread exits at
    ///     its next `recv`.
    ///   * call `debouncer.stop_nonblocking()` — signals the debouncer
    ///     event loop to exit on its next tick and drops the watcher,
    ///     closing the FSEvent / inotify / ReadDirectoryChangesW stream.
    ///   * detach both `JoinHandle`s.
    ///
    /// In-flight handler invocations finish on their own (at most one
    /// more batch) once the caller releases any locks they hold.
    pub fn stop(&mut self) {
        self.watch_tx.take();
        if let Some(debouncer) = self.debouncer.lock().take() {
            debouncer.stop_nonblocking();
        }

        self.owner_thread.take();

        info!("Background file watcher stop signaled");
    }

    /// Queue a non-recursive watch registration on `dir`.
    ///
    /// The owner thread is always blocked on `watch_rx.recv()`, so
    /// the `send()` here wakes it immediately via the channel's
    /// condvar — no external unpark needed.
    ///
    /// Returns `false` once `stop()` has dropped our `Sender` — any
    /// further request is silently discarded.
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

#[tracing::instrument(name = "fs_events", skip(events, shared_picker, shared_frecency), level = Level::DEBUG)]
fn handle_debounced_events(
    events: Vec<DebouncedEvent>,
    git_workdir: &Option<PathBuf>,
    shared_picker: &SharedFilePicker,
    shared_frecency: &SharedFrecency,
    mode: FFFMode,
) -> Vec<PathBuf> {
    // this will be called very often, we have to minimiy the lock time for file picker
    let repo = git_workdir.as_ref().and_then(|p| Repository::open(p).ok());
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
            // Directory-level remove: macOS FSEvents delivers a single
            // `Remove(Folder)` event for a whole directory tree (e.g.
            // after `git reset --hard` wipes a dir full of staged-but-
            // uncommitted files). Individual per-file Remove events for
            // the children do *not* arrive. Treat the folder removal as
            // "evict every indexed descendant".
            let is_folder_removal = matches!(
                debounced_event.event.kind,
                EventKind::Remove(notify::event::RemoveKind::Folder)
            );

            if is_folder_removal {
                dirs_to_remove.push(path.to_path_buf());
            } else if is_removal || !path.exists() {
                paths_to_remove.push(path.as_path());
            } else if path.is_dir() {
                // New directory — collect it so the caller can register a
                // watcher. No filesystem scanning: files that arrive later
                // will be handled by the newly registered watch.
                if !is_path_ignored(path, &repo) {
                    new_dirs_to_watch.push(path.to_path_buf());
                }
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

    // Apply file index updates (add/remove) unconditionally — these must
    // happen even when there is no git repository.
    let (files_to_update_git_status, overflow_count) = if !paths_to_remove.is_empty()
        || !dirs_to_remove.is_empty()
        || !paths_to_add_or_modify.is_empty()
    {
        debug!(
            "Applying file index changes: {} to remove, {} dirs to remove, {} to add/modify",
            paths_to_remove.len(),
            dirs_to_remove.len(),
            paths_to_add_or_modify.len(),
        );

        let apply_changes = |picker: &mut FilePicker| -> (Vec<PathBuf>, usize) {
            // Remove whole directories first so any subsequent single-file
            // remove event for a path that lived under them becomes a cheap
            // no-op rather than a failed lookup.
            for dir in &dirs_to_remove {
                let count = picker.remove_all_files_in_dir(dir);
                debug!("remove_all_files_in_dir({:?}) -> {} files", dir, count);
            }

            for path in &paths_to_remove {
                let removed = picker.remove_file_by_path(path);
                debug!("remove_file_by_path({:?}) -> {}", path, removed);
            }

            let mut files_to_update = Vec::with_capacity(paths_to_add_or_modify.len());
            for path in &paths_to_add_or_modify {
                let added = picker.on_create_or_modify(path).is_some();
                if added {
                    debug!("on_create_or_modify({:?}) -> Some", path);
                    files_to_update.push(path.to_path_buf());
                } else {
                    error!("on_create_or_modify({:?}) -> None (file not added!)", path);
                }
            }
            let overflow_count = picker.get_overflow_files().len();
            info!(
                "apply_changes complete: {} files to update git status, overflow={}",
                files_to_update.len(),
                overflow_count,
            );
            (files_to_update, overflow_count)
        };

        let Ok(mut guard) = shared_picker.write() else {
            error!("Failed to acquire file picker write lock");
            return new_dirs_to_watch;
        };
        let Some(ref mut picker) = *guard else {
            error!("File picker not initialized");
            return new_dirs_to_watch;
        };
        apply_changes(picker)
    } else {
        debug!("No file index changes to apply");
        (Vec::new(), 0)
    };

    // The overflow arena grows monotonically as new files are created — a
    // file's chunks are added on creation but never reclaimed on removal.
    // On directories with high churn (e.g. `$HOME` with editor temp files,
    // browser caches) this inflates RSS unboundedly. Once overflow exceeds
    // the threshold, fall back to a full rescan: that replaces `sync_data`
    // and drops the builder arena, which is the only path that reclaims it.
    if overflow_count > MAX_OVERFLOW_FILES {
        warn!(
            ?overflow_count,
            "Overflow count exceeded the threshold, triggering full rescan.",
        );
        if let Err(e) = shared_picker.trigger_full_rescan_async(shared_frecency) {
            error!("Failed to trigger full rescan: {:?}", e);
        }
        return new_dirs_to_watch;
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

    // Git status updates require a repository.
    let Some(repo) = repo.as_ref() else {
        debug!("No git repo available, skipping git status updates");
        return new_dirs_to_watch;
    };

    if need_full_git_rescan {
        info!("Triggering full git rescan");

        if let Err(e) = shared_picker.refresh_git_status(shared_frecency) {
            error!("Failed to refresh git status: {:?}", e);
        }
        // IMPORTANT: do NOT return here. When a batch contains both
        // `.git/index` events (e.g. from `git add`) AND worktree-file
        // Modify events (e.g. a subsequent edit to the same file),
        // `refresh_git_status` might run while libgit2 sees an
        // intermediate state — lock-wait mitigates this but can't fully
        // eliminate it, and refresh doesn't always observe the final
        // worktree contents if the edit event landed just before the
        // batch flushed. Re-running the per-path query for explicitly
        // changed files overrides any stale bits from refresh with an
        // authoritative per-file status read.
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
                return new_dirs_to_watch;
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

    new_dirs_to_watch
}

/// After registering a watch on a newly created directory, list its
/// immediate children and add any files to the picker.
fn track_files_from_new_directories(
    dir: &Path,
    shared_picker: &SharedFilePicker,
    shared_frecency: &SharedFrecency,
    git_workdir: &Option<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    let repo = git_workdir.as_ref().and_then(|p| Repository::open(p).ok());
    let mut files_to_add = Vec::new();

    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|ft| ft.is_file()) {
            let path = entry.path();
            if should_include_file(&path, &repo) {
                files_to_add.push(path);
            }
        }
    }

    if files_to_add.is_empty() {
        return;
    }

    // brief read lock
    {
        let Ok(mut guard) = shared_picker.write() else {
            return;
        };

        let Some(ref mut picker) = *guard else {
            return;
        };

        for path in &files_to_add {
            picker.on_create_or_modify(path);
        }
    }

    if let Some(repo) = repo.as_ref() {
        let status = match GitStatusCache::git_status_for_paths(repo, &files_to_add) {
            Ok(status) => status,
            Err(e) => {
                tracing::error!(?e, "inject_existing_files: git status query failed");
                return;
            }
        };

        if let Ok(mut guard) = shared_picker.write()
            && let Some(ref mut picker) = *guard
            && let Err(e) = picker.update_git_statuses(status, shared_frecency)
        {
            error!("inject_existing_files: failed to update git statuses: {e:?}");
        }
    }

    debug!(
        "Injected {} existing files from new directory {}",
        files_to_add.len(),
        dir.display(),
    );
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

#[inline]
fn is_path_ignored(path: &Path, repo: &Option<Repository>) -> bool {
    match repo.as_ref() {
        Some(repo) => repo.is_path_ignored(path) == Ok(true),
        None => is_non_code_directory(path),
    }
}

#[inline]
pub(crate) fn is_git_file(path: &Path) -> bool {
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
        // which files are modified/untracked, so we deliberately skip them —
        // watching refs/ recursively would cost one inotify watch per ref
        // namespace on repos with many branches/remotes.
        if path_in_git_dir == Path::new("index") || path_in_git_dir == Path::new("index.lock") {
            return true;
        }
        if path_in_git_dir == Path::new("HEAD") {
            return true;
        }
        if path_in_git_dir == Path::new("info/exclude")
            || path_in_git_dir == Path::new("info/sparse-checkout")
        {
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
    // index, index.lock, HEAD, MERGE_HEAD, CHERRY_PICK_HEAD, REVERT_HEAD.
    // We intentionally do NOT watch refs/ — individual ref updates don't
    // affect worktree status, and a recursive watch there blows up inotify
    // watch counts on repos with many branches/remotes/tags.
    if let Err(e) = debouncer.watch(&git_dir, RecursiveMode::NonRecursive) {
        warn!("Failed to watch .git directory: {}", e);
        return;
    }

    // Watch info/ non-recursively for exclude and sparse-checkout
    let info_dir = git_dir.join("info");
    if info_dir.is_dir()
        && let Err(e) = debouncer.watch(&info_dir, RecursiveMode::NonRecursive)
    {
        warn!("Failed to watch .git/info: {}", e);
    }
}
