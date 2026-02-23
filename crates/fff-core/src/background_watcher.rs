use crate::error::Error;
use crate::file_picker::FilePicker;
use crate::git::GitStatusCache;
use crate::sort_buffer::sort_with_buffer;
use crate::{SharedFrecency, SharedPicker};
use git2::Repository;
use notify::event::{AccessKind, AccessMode};
use notify::{Config, EventKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, DebouncedEvent, NoCache, new_debouncer_opt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{Level, debug, error, info, warn};

type Debouncer = notify_debouncer_full::Debouncer<notify::RecommendedWatcher, NoCache>;

pub struct BackgroundWatcher {
    debouncer: Arc<Mutex<Option<Debouncer>>>,
}

const DEBOUNCE_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_PATHS_THRESHOLD: usize = 1024;
const MAX_SELECTIVE_WATCH_DIRS: usize = 100;

impl BackgroundWatcher {
    pub fn new(
        base_path: PathBuf,
        git_workdir: Option<PathBuf>,
        shared_picker: SharedPicker,
        shared_frecency: SharedFrecency,
    ) -> Result<Self, Error> {
        info!(
            "Initializing background watcher for path: {}",
            base_path.display()
        );

        let debouncer =
            Self::create_debouncer(base_path, git_workdir, shared_picker, shared_frecency)?;
        info!("Background file watcher initialized successfully");

        Ok(Self {
            debouncer: Arc::new(Mutex::new(Some(debouncer))),
        })
    }

    fn create_debouncer(
        base_path: PathBuf,
        git_workdir: Option<PathBuf>,
        shared_picker: SharedPicker,
        shared_frecency: SharedFrecency,
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
                        handle_debounced_events(
                            events,
                            &git_workdir_for_handler,
                            &shared_picker,
                            &shared_frecency,
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

        // Watch only non-ignored directories to avoid flooding the OS event buffer.
        // On macOS, FSEvents has a fixed-size kernel buffer — watching huge gitignored
        // directories like `target/` in rust causes buffer overflow, which drops real source file
        // events. Instead we watch the root non-recursively (for top-level file changes
        // and new directory detection) and each non-ignored subdirectory recursively.
        let watch_dirs = collect_non_ignored_dirs(&base_path);

        if watch_dirs.len() > MAX_SELECTIVE_WATCH_DIRS {
            tracing::warn!(
                "Too many non-ignored directories ({}/{}) can't efficiently watch them",
                watch_dirs.len(),
                MAX_SELECTIVE_WATCH_DIRS
            );
            debouncer.watch(base_path.as_path(), RecursiveMode::Recursive)?;
        } else {
            debouncer.watch(base_path.as_path(), RecursiveMode::NonRecursive)?;

            for dir in &watch_dirs {
                match debouncer.watch(dir.as_path(), RecursiveMode::Recursive) {
                    Ok(()) => {}
                    Err(e) => {
                        // Non-fatal: directory may have been removed between discovery and watch
                        warn!("Failed to watch directory {}: {}", dir.display(), e);
                    }
                }
            }

            // In selective mode the .git directory is excluded from the non-ignored
            // dirs, but we still need to observe changes that affect git status
            // (staging, unstaging, committing, branch switches, merges, etc.).
            watch_git_status_paths(&mut debouncer, git_workdir.as_ref());
        }

        info!(
            "File watcher initialized for {} directories under {}",
            watch_dirs.len(),
            base_path.display()
        );

        Ok(debouncer)
    }

    pub fn stop(&self) {
        if let Ok(Some(debouncer)) = self.debouncer.lock().map(|mut debouncer| debouncer.take()) {
            drop(debouncer);
            info!("Background file watcher stopped successfully");
        } else {
            error!("Failed to stop background watcher");
        }
    }
}

impl Drop for BackgroundWatcher {
    fn drop(&mut self) {
        if let Ok(mut debouncer_guard) = self.debouncer.lock() {
            if let Some(debouncer) = debouncer_guard.take() {
                drop(debouncer);
            }
        } else {
            error!("Failed to acquire debouncer lock to drop");
        }
    }
}

#[tracing::instrument(name = "fs_events", skip(events, shared_picker, shared_frecency), level = Level::DEBUG)]
fn handle_debounced_events(
    events: Vec<DebouncedEvent>,
    git_workdir: &Option<PathBuf>,
    shared_picker: &SharedPicker,
    shared_frecency: &SharedFrecency,
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
                                file.path.display()
                            );
                            files_to_update.push(file.path.clone());
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

    // Git status updates require a repository.
    let Some(repo) = repo.as_ref() else {
        debug!("No git repo available, skipping git status updates");
        return;
    };

    if need_full_git_rescan {
        info!("Triggering full git rescan");

        let result = FilePicker::refresh_git_status(shared_picker, shared_frecency);
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

    // If there is a git repo, respect its ignore rules.
    // If there is no repo (or the check fails), include the file.
    match repo.as_ref() {
        Some(repo) => repo.is_path_ignored(path) != Ok(true),
        None => true,
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

/// Collects immediate non-ignored subdirectories of `base_path` using the `ignore` crate
/// to respect .gitignore, .ignore, and global gitignore rules. This is used to set up
/// selective file watching — only non-ignored directories get a recursive watcher,
/// preventing gitignored directories like `target/` from flooding the OS event buffer.
fn collect_non_ignored_dirs(base_path: &Path) -> Vec<PathBuf> {
    use ignore::WalkBuilder;

    let walker = WalkBuilder::new(base_path)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .follow_links(false)
        .max_depth(Some(1))
        .build();

    let mut dirs = Vec::new();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        let path = entry.path();

        // Skip the root directory itself
        if path == base_path {
            continue;
        }

        if path.is_dir() && !is_git_file(path) {
            dirs.push(path.to_path_buf());
        }
    }

    dirs
}
