use crate::FILE_PICKER;
use crate::error::Error;
use crate::file_picker::FilePicker;
use crate::git::GitStatusCache;
use git2::Repository;
use notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, DebouncedEvent, new_debouncer};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{debug, error, info, warn};

type Debouncer = notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>;

pub struct BackgroundWatcher {
    debouncer: Arc<Mutex<Option<Debouncer>>>,
}

const DEBOUNCE_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_PATHS_THRESHOLD: usize = 50;

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
        let mut debouncer = new_debouncer(DEBOUNCE_TIMEOUT, {
            move |result: DebounceEventResult| match result {
                Ok(events) => {
                    if !events.is_empty() {
                        handle_debounced_events(events, &git_workdir);
                    }
                }
                Err(errors) => {
                    error!("File watcher errors: {:?}", errors);
                }
            }
        })?;

        debouncer
            .watcher()
            .watch(base_path.as_path(), RecursiveMode::Recursive)?;
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

fn handle_debounced_events(events: Vec<DebouncedEvent>, git_workdir: &Option<PathBuf>) {
    debug!("Processing {} debounced events", events.len());

    let Ok(mut file_picker_guard) = FILE_PICKER.write() else {
        error!("Failed to acquire file picker write lock");
        return;
    };

    let Some(ref mut picker) = *file_picker_guard else {
        error!("File picker not initialized");
        return;
    };

    let mut need_full_git_rescan = false;

    let repo = git_workdir.as_ref().and_then(|p| Repository::open(p).ok());
    let mut files_to_update_git_status = Vec::with_capacity(events.len() * 2);
    let mut affected_paths_count = 0usize;

    for event in &events {
        let path = &event.path;
        if is_ignore_definition_path(path) {
            info!(
                "Detected change in the ignore definition file: {}",
                path.display()
            );

            return trigger_full_rescan(picker);
        }

        if is_dotgit_change_affecting_status(path, &repo) {
            need_full_git_rescan = true;
        }

        if !should_include_file(path, &repo) {
            continue;
        }

        debug!("Handling fs event: {:?}", event);

        affected_paths_count += 1;
        if affected_paths_count > MAX_PATHS_THRESHOLD {
            warn!(
                "Too many affected paths ({}) in a single batch, triggering full rescan",
                affected_paths_count
            );

            return trigger_full_rescan(picker);
        }

        if !path.exists() {
            picker.remove_file_by_path(path);
            continue;
        }

        let file = picker.on_create_or_modify(path);
        if let Some(file) = file {
            files_to_update_git_status.push(file.relative_path.clone());
        }
    }

    if need_full_git_rescan {
        drop(file_picker_guard); // it's going to be relocked after rescan
        info!("Triggering full git rescan by the notification results");

        if let Err(e) = FilePicker::refresh_git_status_global() {
            error!("Failed to refresh git status: {:?}", e);
        }
    } else if let Some(repo) = repo.as_ref() {
        let status = GitStatusCache::git_status_for_paths(repo, &files_to_update_git_status);
        if let Err(e) = picker.update_git_statuses(status) {
            error!("Failed to update git statuses: {:?}", e);
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

fn trigger_full_rescan(picker: &mut FilePicker) {
    if let Err(e) = picker.trigger_rescan() {
        error!("Failed to trigger full rescan: {:?}", e);
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
