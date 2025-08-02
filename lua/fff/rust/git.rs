use git2::{Repository, Status, StatusOptions};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use tracing::{error, info};

#[derive(Debug, Clone)]
pub struct GitStatusCache {
    paths: Vec<PathBuf>,
    statuses: Vec<Status>,
}

impl GitStatusCache {
    #[allow(dead_code)]
    fn new() -> Self {
        Self {
            paths: Vec::new(),
            statuses: Vec::new(),
        }
    }

    fn from_git_entries(mut entries: Vec<(PathBuf, Status)>) -> Self {
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        let (paths, statuses) = entries.into_iter().unzip();
        Self { paths, statuses }
    }

    pub fn lookup_status(&self, full_path: &Path) -> Option<Status> {
        match self
            .paths
            .binary_search_by(|probe| probe.as_path().cmp(full_path))
        {
            Ok(idx) => self.statuses.get(idx).copied(),
            Err(_) => None,
        }
    }

    pub fn read_git_status(git_workdir: Option<&PathBuf>) -> Option<Self> {
        let git_start = std::time::Instant::now();
        info!("GIT: Starting git status read");

        let mut entries = Vec::new();
        let git_workdir = git_workdir.as_ref()?;
        let repository = Repository::open(git_workdir).ok()?;

        let status_start = std::time::Instant::now();
        let statuses = repository
            .statuses(Some(&mut StatusOptions::new().include_untracked(true)))
            .map_err(|e| {
                error!("Failed to get git statuses: {}", e);
                e
            })
            .ok()?;
        let status_time = status_start.elapsed();
        info!("GIT: Status query completed in {:?}", status_time);

        info!(
            "GIT STATUS: {} git entries in {}",
            statuses.len(),
            git_workdir.display()
        );

        for entry in &statuses {
            if let Some(entry_path) = entry.path() {
                let full_path = git_workdir.join(entry_path);
                entries.push((full_path, entry.status()));
            }
        }

        let total_time = git_start.elapsed();
        info!("GIT: Total git status read time {:?}", total_time);

        Some(Self::from_git_entries(entries))
    }

    pub fn read_git_status_parallel(git_workdir: Option<PathBuf>) -> JoinHandle<Option<Self>> {
        thread::spawn(move || Self::read_git_status(git_workdir.as_ref()))
    }
}

#[inline]
#[allow(dead_code)]
pub fn is_modified_status(status: Status) -> bool {
    status.intersects(
        Status::WT_MODIFIED
            | Status::INDEX_MODIFIED
            | Status::WT_NEW
            | Status::INDEX_NEW
            | Status::WT_RENAMED,
    )
}

pub fn format_git_status(status: Option<Status>) -> &'static str {
    match status {
        None => "clear",
        Some(status) => {
            if status.contains(Status::WT_NEW) {
                "untracked"
            } else if status.contains(Status::WT_MODIFIED) {
                "modified"
            } else if status.contains(Status::WT_DELETED) {
                "deleted"
            } else if status.contains(Status::WT_RENAMED) {
                "renamed"
            } else if status.contains(Status::INDEX_NEW) {
                "staged_new"
            } else if status.contains(Status::INDEX_MODIFIED) {
                "staged_modified"
            } else if status.contains(Status::INDEX_DELETED) {
                "staged_deleted"
            } else if status.contains(Status::IGNORED) {
                "ignored"
            } else if status.contains(Status::CURRENT) || status.is_empty() {
                "clean"
            } else {
                "unknown"
            }
        }
    }
}
