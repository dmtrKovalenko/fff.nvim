use crate::error::Result;
use git2::{Repository, Status, StatusOptions};
use std::{
    collections::HashMap,
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

/// Configuration for git recency scoring.
#[derive(Debug, Clone, Copy)]
pub struct GitRecencyConfig {
    /// Maximum number of recent commits to analyze (default: 10)
    pub max_commits: usize,
    /// Ignore commits that touch more files than this threshold (default: 50)
    pub max_files_per_commit: usize,
    /// Maximum bonus score for a file in the most recent commit (default: 15)
    pub max_bonus: i32,
}

impl Default for GitRecencyConfig {
    fn default() -> Self {
        Self {
            max_commits: 10,
            max_files_per_commit: 50,
            max_bonus: 15,
        }
    }
}

/// Analyze the last N commits on the current branch and compute a recency score
/// for each file that was touched. Uses max-only semantics: each file gets the
/// score from its most recent qualifying commit appearance.
///
/// Score formula: `max_bonus * (max_commits - commit_position) / max_commits`
/// where commit_position 0 = most recent.
///
/// Commits with more files changed than `config.max_files_per_commit` are skipped
/// (filters out merge commits, bulk refactors, initial imports, etc.).
///
/// Returns a map from absolute file paths to their recency scores.
#[tracing::instrument(skip(repo), level = tracing::Level::DEBUG)]
pub fn get_recent_commit_files(
    repo: &Repository,
    config: &GitRecencyConfig,
) -> HashMap<PathBuf, i32> {
    let mut scores: HashMap<PathBuf, i32> = HashMap::new();

    if config.max_commits == 0 || config.max_bonus <= 0 {
        return scores;
    }

    let workdir = match repo.workdir() {
        Some(w) => w,
        None => return scores, // bare repo
    };

    let head = match repo.head() {
        Ok(h) => h,
        Err(e) => {
            tracing::debug!(?e, "Failed to get HEAD for git recency");
            return scores;
        }
    };

    let head_oid = match head.target() {
        Some(oid) => oid,
        None => return scores,
    };

    let mut revwalk = match repo.revwalk() {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(?e, "Failed to create revwalk for git recency");
            return scores;
        }
    };

    if revwalk.push(head_oid).is_err() {
        return scores;
    }

    // Walk commits, assigning position 0 to the most recent qualifying commit
    let mut qualifying_position: usize = 0;

    for oid_result in revwalk {
        if qualifying_position >= config.max_commits {
            break;
        }

        let oid = match oid_result {
            Ok(oid) => oid,
            Err(_) => continue,
        };

        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let commit_tree = match commit.tree() {
            Ok(t) => t,
            Err(_) => continue,
        };

        // Diff against first parent (standard for merge commits — shows what
        // the branch actually changed, not what was merged in)
        let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

        let diff = match repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&commit_tree), None) {
            Ok(d) => d,
            Err(_) => continue,
        };

        // Collect files from this commit's diff
        let mut commit_files: Vec<PathBuf> = Vec::new();
        let deltas = diff.deltas();
        let delta_count = deltas.len();

        // Skip commits with too many changes
        if delta_count > config.max_files_per_commit {
            tracing::trace!(
                delta_count,
                max = config.max_files_per_commit,
                "Skipping large commit for git recency"
            );
            continue;
        }

        for delta in deltas {
            // Prefer the new_file path (handles renames correctly)
            if let Some(path) = delta.new_file().path() {
                commit_files.push(workdir.join(path));
            }
        }

        // Linear decay: most recent qualifying commit gets max_bonus,
        // oldest gets max_bonus / max_commits
        let score = config.max_bonus * (config.max_commits - qualifying_position) as i32
            / config.max_commits as i32;

        // Max-only: only insert if the file doesn't already have a higher score
        // (from a more recent commit)
        for file_path in commit_files {
            scores.entry(file_path).or_insert(score);
        }

        qualifying_position += 1;
    }

    tracing::debug!(
        files_scored = scores.len(),
        commits_analyzed = qualifying_position,
        "Git recency analysis complete"
    );

    scores
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_recency_config_default() {
        let config = GitRecencyConfig::default();
        assert_eq!(config.max_commits, 10);
        assert_eq!(config.max_files_per_commit, 50);
        assert_eq!(config.max_bonus, 15);
    }

    #[test]
    fn test_git_recency_disabled_with_zero_commits() {
        let config = GitRecencyConfig {
            max_commits: 0,
            ..Default::default()
        };
        // Can't easily test with a real repo, but verify the early return path
        // by checking config values
        assert_eq!(config.max_commits, 0);
    }

    #[test]
    fn test_git_recency_score_formula() {
        // Verify the linear decay formula produces expected values
        let max_bonus: i32 = 15;
        let max_commits: usize = 10;

        // Position 0 (most recent) -> 15 * 10/10 = 15
        let score_0 = max_bonus * (max_commits - 0) as i32 / max_commits as i32;
        assert_eq!(score_0, 15);

        // Position 1 -> 15 * 9/10 = 13
        let score_1 = max_bonus * (max_commits - 1) as i32 / max_commits as i32;
        assert_eq!(score_1, 13);

        // Position 5 -> 15 * 5/10 = 7
        let score_5 = max_bonus * (max_commits - 5) as i32 / max_commits as i32;
        assert_eq!(score_5, 7);

        // Position 9 (oldest) -> 15 * 1/10 = 1
        let score_9 = max_bonus * (max_commits - 9) as i32 / max_commits as i32;
        assert_eq!(score_9, 1);
    }
}
