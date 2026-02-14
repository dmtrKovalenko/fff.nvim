use crate::FILE_PICKER;
use crate::MMAP_CACHE;
use crate::error::Error;
use crate::file_picker::FilePicker;
use crate::git::GitStatusCache;
use crate::sort_buffer::sort_with_buffer;
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

impl BackgroundWatcher {
    pub fn new(base_path: PathBuf, git_workdir: Option<PathBuf>) -> Result<Self, Error> {
        info!(
            "Initializing background watcher for path: {}",
            base_path.display()
        );

        let debouncer = Self::create_debouncer(base_path, git_workdir)?;
        info!("Background file watcher initialized successfully");

        Ok(Self {
            debouncer: Arc::new(Mutex::new(Some(debouncer))),
        })
    }

    fn create_debouncer(
        base_path: PathBuf,
        git_workdir: Option<PathBuf>,
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
                        handle_debounced_events(events, &git_workdir);
                    }
                    Err(errors) => {
                        error!("File watcher errors: {:?}", errors);
                    }
                }
            },
            RecommendedCache::new(),
            config,
        )?;

        debouncer.watch(base_path.as_path(), RecursiveMode::Recursive)?;
        info!("File watcher initizlieed for path: {}", base_path.display());

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

#[tracing::instrument(name = "fs_events", skip(events), level = Level::DEBUG)]
fn handle_debounced_events(events: Vec<DebouncedEvent>, git_workdir: &Option<PathBuf>) {
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
        trigger_full_rescan();
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

        if let Err(e) = FilePicker::refresh_git_status_global() {
            error!("Failed to refresh git status: {:?}", e);
        }

        return;
    }

    if paths_to_remove.is_empty() && paths_to_add_or_modify.is_empty() {
        return;
    }

    let files_to_update_git_status = {
        let Ok(mut file_picker_guard) = FILE_PICKER.write() else {
            error!("Failed to acquire file picker write lock");
            return;
        };

        let Some(ref mut picker) = *file_picker_guard else {
            error!("File picker not initialized");
            return;
        };

        // Apply file removals
        for path in paths_to_remove {
            picker.remove_file_by_path(path);
            MMAP_CACHE.invalidate(path);
        }

        // Apply file additions/modifications and collect paths for git status update
        let mut files_to_update_git_status = Vec::with_capacity(paths_to_add_or_modify.len());
        for path in paths_to_add_or_modify {
            MMAP_CACHE.invalidate(path);
            if let Some(file) = picker.on_create_or_modify(path) {
                files_to_update_git_status.push(file.path.clone());
            }
        }

        files_to_update_git_status
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

    // only lock the picker for theshortest possitble time
    if let Ok(mut file_picker_guard) = FILE_PICKER.write()
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

fn trigger_full_rescan() {
    info!("Triggering full filesystem rescan");

    // Clear mmap cache since file contents may have changed
    MMAP_CACHE.clear();

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
