use crate::error::Result;
use git2::{Repository, Status, StatusOptions};
use std::{
    fmt::Debug,
    path::{Path, PathBuf},
};
use tracing::debug;

/// Represents a cache of a single git status query, if there is no
/// status aka file is clear but it was specifically requested to updated
/// the status is `None` otherwise contains only actual file statuses.
#[derive(Debug, Clone)]
pub struct GitStatusCache(Vec<(PathBuf, Status)>);

impl IntoIterator for GitStatusCache {
    type Item = (PathBuf, Status);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl GitStatusCache {
    pub fn statuses_len(&self) -> usize {
        self.0.len()
    }

    pub fn lookup_status(&self, full_path: &Path) -> Option<Status> {
        self.0
            .binary_search_by(|(path, _)| path.as_path().cmp(full_path))
            .ok()
            .and_then(|idx| self.0.get(idx).map(|(_, status)| *status))
    }

    #[tracing::instrument(skip(repo, status_options))]
    fn read_status_impl(repo: &Repository, status_options: &mut StatusOptions) -> Result<Self> {
        let statuses = repo.statuses(Some(status_options))?;
        let Some(repo_path) = repo.workdir() else {
            return Ok(Self(vec![])); // repo is bare
        };

        let mut entries = Vec::with_capacity(statuses.len());
        for entry in &statuses {
            if let Some(entry_path) = entry.path() {
                let full_path = repo_path.join(entry_path);
                entries.push((full_path, entry.status()));
            }
        }

        Ok(Self(entries))
    }

    pub fn read_git_status(
        git_workdir: Option<&Path>,
        status_options: &mut StatusOptions,
    ) -> Option<Self> {
        let git_workdir = git_workdir.as_ref()?;
        let repository = Repository::open(git_workdir).ok()?;

        let status = Self::read_status_impl(&repository, status_options);

        match status {
            Ok(status) => Some(status),
            Err(e) => {
                tracing::error!(?e, "Failed to read git status");

                None
            }
        }
    }

    #[tracing::instrument(skip(repo), level = tracing::Level::DEBUG)]
    pub fn git_status_for_paths<TPath: AsRef<Path> + Debug>(
        repo: &Repository,
        paths: &[TPath],
    ) -> Result<Self> {
        if paths.is_empty() {
            return Ok(Self(vec![]));
        }

        let Some(workdir) = repo.workdir() else {
            return Ok(Self(vec![]));
        };

        // git pathspec is pretty slow and requires to walk the whole directory
        // so for a single file which is the most general use case we query directly the file
        if paths.len() == 1 {
            let full_path = paths[0].as_ref();
            let relative_path = full_path.strip_prefix(workdir)?;
            let status = repo.status_file(relative_path)?;

            return Ok(Self(vec![(full_path.to_path_buf(), status)]));
        }

        let mut status_options = StatusOptions::new();
        status_options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            // when reading partial status it's important to include all files requested
            .include_unmodified(true);

        for path in paths {
            status_options.pathspec(path.as_ref().strip_prefix(workdir)?);
        }

        let git_status_cache = Self::read_status_impl(repo, &mut status_options)?;
        debug!(
            status_len = git_status_cache.statuses_len(),
            "Multiple files git status"
        );

        Ok(git_status_cache)
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
        None => "clean",
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
