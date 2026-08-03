use git2::{Config, Repository};
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub(crate) struct GitIgnorePolicy {
    pub(crate) base_document: String,
    pub(crate) ignore_files: Vec<PathBuf>,
    pub(crate) sources: Vec<PathBuf>,
}

impl GitIgnorePolicy {
    pub(crate) fn discover(base_path: &Path) -> Self {
        let repo = Repository::discover(base_path).ok();
        // User excludes are process-wide Git policy, not repository metadata.
        // `open_default` keeps non-Git vaults on the same contract as Git roots.
        let global = repo
            .as_ref()
            .map(Repository::config)
            .unwrap_or_else(Config::open_default)
            .and_then(|mut config| config.snapshot())
            .and_then(|config| config.get_path("core.excludesFile"))
            .ok()
            .or_else(default_global_excludes_path);

        let mut policy = Self::default();
        if let Some(path) = global {
            policy.add_source(path);
        }
        if let Some(repo) = &repo {
            // Linked worktrees share this file with the primary worktree.
            policy.add_source(repo.commondir().join("info/exclude"));
        }
        policy.sources.extend(config_sources(repo.as_ref()));
        policy.sources.sort_unstable();
        policy.sources.dedup();
        policy
    }

    fn add_source(&mut self, path: PathBuf) {
        if let Ok(content) = std::fs::read_to_string(&path) {
            self.base_document.push_str(&content);
            if !content.ends_with('\n') {
                self.base_document.push('\n');
            }
            self.ignore_files.push(path.clone());
        }
        self.sources.push(path);
    }

    #[cfg(feature = "zlob")]
    pub(crate) fn patterns(&self) -> impl Iterator<Item = &str> {
        self.base_document.lines()
    }
}

fn default_global_excludes_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .map(|config| config.join("git/ignore"))
}

fn config_sources(repo: Option<&Repository>) -> Vec<PathBuf> {
    let mut paths = [
        Config::find_system().ok(),
        Config::find_global().ok(),
        Config::find_xdg().ok(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if let Some(repo) = repo {
        paths.push(repo.commondir().join("config"));
        // extensions.worktreeConfig stores per-worktree overrides here.
        paths.push(repo.path().join("config.worktree"));
    }
    paths.sort_unstable();
    paths.dedup();
    paths
}

/// Directories excluded when walking a non-git root. Entries are `cfg`-gated
/// so a single iteration covers standard + platform-specific overrides.
pub(crate) const IGNORED_DIRS: &[&str] = &[
    "node_modules",
    "__pycache__",
    "venv",
    ".venv",
    // Rust (glob-only patterns for non_git_repo_overrides; is_non_code_directory
    // matches the "target" component separately).
    "target/debug",
    "target/release",
    "target/rust-analyzer",
    "target/criterion",
    #[cfg(target_os = "macos")]
    "Library/Application Support",
    #[cfg(target_os = "macos")]
    "Library/Caches",
    // App-group sandbox storage — used by iMessage, Photos, Notes, Calendar,
    // Electron apps, etc. for SQLite-WAL, LevelDB, protobuf files. These are
    // almost entirely extension-less binary files (~80k on a typical $HOME)
    // that never need to appear in a fuzzy or grep search.
    #[cfg(target_os = "macos")]
    "Library/Group Containers",
    #[cfg(target_os = "macos")]
    "Library/Containers",
    #[cfg(target_os = "windows")]
    "bin/Debug",
    #[cfg(target_os = "windows")]
    "bin/Release",
    #[cfg(target_os = "windows")]
    "Program Files",
    #[cfg(target_os = "windows")]
    "Program Files (x86)",
    #[cfg(target_os = "windows")]
    "AppData/Local",
    #[cfg(target_os = "windows")]
    "AppData/Roaming",
];

#[cfg(all(not(feature = "zlob"), feature = "ripgrep"))]
pub(crate) fn non_git_repo_overrides(base_path: &Path) -> Option<ignore::overrides::Override> {
    use ignore::overrides::OverrideBuilder;

    let mut builder = OverrideBuilder::new(base_path);
    for dir in IGNORED_DIRS {
        let pattern = format!("!**/{dir}/");
        if let Err(e) = builder.add(&pattern) {
            tracing::warn!("failed to add ignore pattern {pattern}: {e}");
        }
    }

    builder.build().ok()
}

pub(crate) fn is_non_code_directory(path: &Path) -> bool {
    let path_str = path.as_os_str().to_str().unwrap_or("");
    IGNORED_DIRS.iter().any(|&dir| {
        #[cfg(target_os = "windows")]
        let dir = dir.replace('/', std::path::MAIN_SEPARATOR_STR);
        #[cfg(target_os = "windows")]
        return path_str.contains(dir.as_str());

        #[cfg(not(target_os = "windows"))]
        path_str.contains(dir)
    })
}

#[cfg(test)]
mod tests {
    use super::GitIgnorePolicy;
    use std::fs;

    #[test]
    fn policy_document_orders_global_before_info() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global-ignore");
        let info = dir.path().join("info-exclude");
        fs::write(&global, "*.tmp").unwrap();
        fs::write(&info, "!keep.tmp\n").unwrap();

        let mut policy = GitIgnorePolicy::default();
        policy.add_source(global.clone());
        policy.add_source(info.clone());

        assert_eq!(policy.base_document, "*.tmp\n!keep.tmp\n");
        assert_eq!(policy.ignore_files, vec![global.clone(), info.clone()]);
        assert_eq!(policy.sources, vec![global, info]);
    }

    #[test]
    fn missing_policy_source_is_watched_but_not_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("future-ignore");

        let mut policy = GitIgnorePolicy::default();
        policy.add_source(missing.clone());

        assert!(policy.base_document.is_empty());
        assert!(policy.ignore_files.is_empty());
        assert_eq!(policy.sources, vec![missing]);
    }
}
