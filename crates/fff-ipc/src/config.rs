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
/// # db = "~/.local/share/fff/frecency/"  # defaults to XDG data dir
/// ```
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FffConfig {
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub index: IndexConfig,
    #[serde(default)]
    pub frecency: FrecencyConfig,
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
            level: "info".into(),
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
    /// Default: `$XDG_DATA_HOME/fff/frecency/` → `~/.local/share/fff/frecency/`.
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
