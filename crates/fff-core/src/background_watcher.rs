use crate::FILE_PICKER;
use crate::error::Error;
use crate::file_picker::FilePicker;
use crate::git::GitStatusCache;
use crate::sort_buffer::sort_with_buffer;
use crate::{SharedFrecency, SharedPicker};
use git2::Repository;
use notify::event::{AccessKind, AccessMode};
use notify::{Config, EventKind, RecursiveMode};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, RecommendedCache, new_debouncer_opt,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{Level, error, info, warn};

type Debouncer = notify_debouncer_full::Debouncer<notify::RecommendedWatcher, RecommendedCache>;

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
        shared_picker: Option<SharedPicker>,
        shared_frecency: Option<SharedFrecency>,
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
        shared_picker: Option<SharedPicker>,
        shared_frecency: Option<SharedFrecency>,
    ) -> Result<Debouncer, Error> {
        // do not follow symlinks as then notifiers spawns a bunch of events for symlinked
        // files that could be git ignored, we have to property differentiate those and if
        // the file was edited through a
        let config = Config::default().with_follow_symlinks(false);

        let mut debouncer = new_debouncer_opt(
            DEBOUNCE_TIMEOUT,
            Some(DEBOUNCE_TIMEOUT / 2), // tick rate for the event span
            {
                move |result: DebounceEventResult| match result {
                    Ok(events) => {
                        handle_debounced_events(
                            events,
                            &git_workdir,
                            shared_picker.as_ref(),
                            shared_frecency.as_ref(),
                        );
                    }
                    Err(errors) => {
                        error!("File watcher errors: {:?}", errors);
                    }
                }
            },
            RecommendedCache::new(),
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
    shared_picker: Option<&SharedPicker>,
    shared_frecency: Option<&SharedFrecency>,
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

            if !should_include_file(path, &repo) {
                continue;
            }

            if !path.exists() {
                paths_to_remove.push(path.as_path());
            } else {
                paths_to_add_or_modify.push(path.as_path());
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
        trigger_full_rescan(shared_picker);
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

    let Some(repo) = repo.as_ref() else {
        info!("No git repo, skipping git status updates");
        return;
    };

    if need_full_git_rescan {
        info!("Triggering full git rescan");

        let result = if let Some(sp) = shared_picker {
            FilePicker::refresh_git_status_shared(sp)
        } else {
            FilePicker::refresh_git_status_global()
        };
        if let Err(e) = result {
            error!("Failed to refresh git status: {:?}", e);
        }

        return;
    }

    if paths_to_remove.is_empty() && paths_to_add_or_modify.is_empty() {
        return;
    }

    let apply_changes = |picker: &mut FilePicker| -> Vec<PathBuf> {
        for path in &paths_to_remove {
            picker.remove_file_by_path(path);
        }

        let mut files_to_update = Vec::with_capacity(paths_to_add_or_modify.len());
        for path in &paths_to_add_or_modify {
            if let Some(file) = picker.on_create_or_modify(path) {
                files_to_update.push(file.path.clone());
            }
        }
        files_to_update
    };

    let files_to_update_git_status = if let Some(sp) = shared_picker {
        let Ok(mut guard) = sp.write() else {
            error!("Failed to acquire file picker write lock");
            return;
        };
        let Some(ref mut picker) = *guard else {
            error!("File picker not initialized");
            return;
        };
        apply_changes(picker)
    } else {
        let Ok(mut file_picker_guard) = FILE_PICKER.write() else {
            error!("Failed to acquire file picker write lock");
            return;
        };
        let Some(ref mut picker) = *file_picker_guard else {
            error!("File picker not initialized");
            return;
        };
        apply_changes(picker)
    };

    info!(
        "Fetching git status for {} files",
        files_to_update_git_status.len()
    );

    let status = match GitStatusCache::git_status_for_paths(repo, &files_to_update_git_status) {
        Ok(status) => status,
        Err(e) => {
            tracing::error!(?e, "Failed to query git statue");
            return;
        }
    };

    // only lock the picker for the shortest possible time
    if let Some(sp) = shared_picker {
        if let Ok(mut guard) = sp.write()
            && let Some(ref mut picker) = *guard
        {
            if let Err(e) = picker.update_git_statuses_with_frecency(status, shared_frecency) {
                error!("Failed to update git statuses: {:?}", e);
            } else {
                info!("Successfully updated git statuses in picker");
            }
        } else {
            error!("Failed to acquire picker lock for git status update");
        }
    } else if let Ok(mut file_picker_guard) = FILE_PICKER.write()
        && let Some(ref mut picker) = *file_picker_guard
    {
        if let Err(e) = picker.update_git_statuses(status) {
            error!("Failed to update git statuses: {:?}", e);
        } else {
            info!("Successfully updated git statuses in picker");
        }
    } else {
        error!("Failed to acquire picker lock for git status update");
    }
}

fn trigger_full_rescan(shared_picker: Option<&SharedPicker>) {
    info!("Triggering full filesystem rescan");

    // Note: no need to clear mmaps — they are backed by the kernel page cache
    // and automatically reflect file changes. Old FileItems (and their mmaps)
    // are dropped when the picker rebuilds its file list.

    if let Some(sp) = shared_picker {
        let Ok(mut guard) = sp.write() else {
            error!("Failed to acquire file picker write lock for full rescan");
            return;
        };
        let Some(ref mut picker) = *guard else {
            error!("File picker not initialized, cannot trigger rescan");
            return;
        };
        if let Err(e) = picker.trigger_rescan() {
            error!("Failed to trigger full rescan: {:?}", e);
        } else {
            info!("Full filesystem rescan completed successfully");
        }
    } else {
        let Ok(mut file_picker_guard) = FILE_PICKER.write() else {
            error!("Failed to acquire file picker write lock for full rescan");
            return;
        };
        let Some(ref mut picker) = *file_picker_guard else {
            error!("File picker not initialized, cannot trigger rescan");
            return;
        };
        if let Err(e) = picker.trigger_rescan() {
            error!("Failed to trigger full rescan: {:?}", e);
        } else {
            info!("Full filesystem rescan completed successfully");
        }
    }
}

fn should_include_file(path: &Path, repo: &Option<Repository>) -> bool {
    if !path.is_file() || is_git_file(path) {
        return false;
    }

    repo.as_ref()
        .is_some_and(|repo| repo.is_path_ignored(path) == Ok(false))
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
