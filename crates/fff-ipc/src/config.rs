use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Shared configuration for fff-engine and fff-mcp.
///
/// Loaded from `$XDG_CONFIG_HOME/fff/config.toml`, falling back to
/// `$HOME/.config/fff/config.toml`. CLI flags always override config values.
///
/// Example config file:
/// ```toml
/// [log]
/// level = "debug"
/// # file = "~/.cache/fff_engine.log"  # defaults to ~/.cache/fff_{binary}.log
///
/// [index]
/// no_watch = false
/// no_warmup = false
/// max_cached_files = 30000
///
/// [frecency]
/// # db = "~/.local/share/fff/frecency/"  # set to share one DB across projects
/// ```
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FffConfig {
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub index: IndexConfig,
    #[serde(default)]
    pub frecency: FrecencyConfig,
    #[serde(default)]
    pub worker: WorkerConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerConfig {
    /// Minimum number of worker processes to keep alive.
    pub n_min: u32,
    /// Maximum number of worker processes to spawn.
    pub n_max: u32,
    /// Maximum roots loaded per worker before a new worker is spawned.
    pub roots_per_worker_max: u32,
    /// Seconds a worker with no loaded roots waits before being shut down.
    pub idle_ttl_secs: u64,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            n_min: 1,
            n_max: 4,
            roots_per_worker_max: 8,
            idle_ttl_secs: 300,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LogConfig {
    /// Log level: trace, debug, info, warn, error. Default: info.
    pub level: String,
    /// Override the log file path. Default: `~/.cache/fff_{binary}.log`.
    pub file: Option<String>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            // Target our crates at info; suppress library noise at warn.
            // Use RUST_LOG-style syntax for finer control:
            //   "fff_engine=debug,fff_mcp=debug,warn"
            level: "fff_engine=info,fff_mcp=info,warn".into(),
            file: None,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct IndexConfig {
    /// Disable the background filesystem watcher.
    #[serde(default)]
    pub no_watch: bool,
    /// Skip mmap warmup after initial scan.
    #[serde(default)]
    pub no_warmup: bool,
    /// Maximum number of files to keep content-cached in memory.
    pub max_cached_files: Option<usize>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FrecencyConfig {
    /// Path to the LMDB frecency database directory.
    /// Default: a per-base-path subdirectory under
    /// `$XDG_DATA_HOME/fff/frecency/<slug>/` (the slug is a stable hash of
    /// the canonical base-path). Set this to a fixed directory to share one
    /// DB across all projects — useful when you want cross-project frecency
    /// signal, at the cost of a global size-cap blast radius.
    pub db: Option<String>,
}

/// Returns the config file path:
/// `$XDG_CONFIG_HOME/fff/config.toml` or `$HOME/.config/fff/config.toml`.
///
/// Does not use `dirs::config_dir()` — that returns `~/Library/Application Support`
/// on macOS instead of the XDG-conventional `~/.config`.
pub fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(
                std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .unwrap_or_else(|_| "/tmp".to_string()),
            )
            .join(".config")
        });
    base.join("fff").join("config.toml")
}

/// Load the config file. Returns `FffConfig::default()` when the file is absent.
/// Logs a warning to stderr (not tracing — tracing may not be initialised yet)
/// if the file exists but cannot be parsed.
pub fn load() -> FffConfig {
    let path = config_path();
    if !path.exists() {
        return FffConfig::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(contents) => toml::from_str(&contents).unwrap_or_else(|e| {
            eprintln!(
                "Warning: failed to parse fff config at {}: {e}",
                path.display()
            );
            FffConfig::default()
        }),
        Err(e) => {
            eprintln!(
                "Warning: failed to read fff config at {}: {e}",
                path.display()
            );
            FffConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_config_defaults() {
        let c = WorkerConfig::default();
        assert_eq!(c.n_min, 1);
        assert_eq!(c.n_max, 4);
        assert_eq!(c.roots_per_worker_max, 8);
        assert_eq!(c.idle_ttl_secs, 300);
    }

    #[test]
    fn fff_config_without_worker_section_uses_defaults() {
        let toml = "[log]\nlevel = \"debug\"\n";
        let cfg: FffConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.worker.n_min, 1);
        assert_eq!(cfg.worker.n_max, 4);
    }

    #[test]
    fn fff_config_with_worker_section_parses_fields() {
        let toml = "[worker]\nn_min = 2\nn_max = 8\nroots_per_worker_max = 16\nidle_ttl_secs = 600\n";
        let cfg: FffConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.worker.n_min, 2);
        assert_eq!(cfg.worker.n_max, 8);
        assert_eq!(cfg.worker.roots_per_worker_max, 16);
        assert_eq!(cfg.worker.idle_ttl_secs, 600);
    }
}
