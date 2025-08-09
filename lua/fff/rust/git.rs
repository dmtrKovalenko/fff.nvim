use git2::{Repository, Status, StatusOptions};
use std::{
    fmt::Debug,
    path::{Path, PathBuf},
};
use tracing::{debug, error, info};

#[derive(Debug, Clone)]
pub struct GitStatusCache {
    paths: Vec<PathBuf>,
    statuses: Vec<Status>,
}

impl GitStatusCache {
    fn from_git_entries(mut entries: Vec<(PathBuf, Status)>) -> Self {
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        let (paths, statuses) = entries.into_iter().unzip();
        Self { paths, statuses }
    }

    pub fn lookup_status(&self, full_path: &Path) -> Option<Status> {
        self.paths
            .binary_search_by(|probe| probe.as_path().cmp(full_path))
            .ok()
            .and_then(|idx| self.statuses.get(idx).copied())
    }

    fn read_status_impl(repo: &Repository, status_options: &mut StatusOptions) -> Option<Self> {
        let status_start = std::time::Instant::now();
        info!("GIT: Reading git status");
        let statuses = repo
            .statuses(Some(status_options))
            .map_err(|e| {
                error!("Failed to get git statuses: {}", e);
                e
            })
            .ok()?;
        let status_time = status_start.elapsed();
        let repo_path = repo.path().parent()?;
        info!("GIT: Status query completed in {:?}", status_time);

        let mut entries = Vec::with_capacity(statuses.len());
        for entry in &statuses {
            if let Some(entry_path) = entry.path() {
                let full_path = repo_path.join(entry_path);
                entries.push((full_path, entry.status()));
            }
        }

        Some(Self::from_git_entries(entries))
    }

    pub fn read_git_status(git_workdir: Option<&Path>) -> Option<Self> {
        let git_workdir = git_workdir.as_ref()?;
        let repository = Repository::open(git_workdir).ok()?;

        Self::read_status_impl(
            &repository,
            StatusOptions::new()
                .include_untracked(true)
                .recurse_untracked_dirs(true),
        )
    }

    pub fn git_status_for_paths<TPath: AsRef<Path> + Debug>(
        repo: &Repository,
        paths: &[TPath],
    ) -> Option<Self> {
        let mut status_options = StatusOptions::new();
        debug!(?paths, "Git partial git status for paths");

        status_options
            .include_untracked(true)
            .recurse_untracked_dirs(true);

        for path in paths {
            status_options.pathspec(path.as_ref());
        }

        Self::read_status_impl(repo, &mut status_options)
    }
}

#[inline]
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
