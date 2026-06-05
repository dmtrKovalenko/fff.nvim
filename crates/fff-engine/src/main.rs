mod handlers;
mod lifecycle;
mod server;
mod state;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// fff singleton search engine daemon.
///
/// One instance per project root. fff-mcp instances connect as stateless
/// proxies; this daemon owns FilePicker, BigramFilter, and FrecencyTracker.
#[derive(Parser, Debug)]
#[command(name = "fff-engine", version)]
pub(crate) struct Args {
    /// Project root to index. Required — fff-mcp derives the socket path from
    /// the canonical form of this path.
    #[arg(long = "base-path", value_name = "PATH")]
    pub base_path: PathBuf,

    /// Path to the LMDB frecency database directory.
    /// Defaults to `$XDG_DATA_HOME/fff/frecency/` (R5: frecency enabled by default).
    #[arg(long = "frecency-db", value_name = "PATH")]
    pub frecency_db_path: Option<PathBuf>,

    /// Path to the log file.
    #[arg(long = "log-file", value_name = "PATH")]
    pub log_file: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error). Default: info.
    #[arg(long = "log-level", default_value = "info")]
    pub log_level: String,

    /// Disable the background filesystem watcher. Files are scanned once at
    /// startup but not monitored for changes. Useful for tests.
    #[arg(long = "no-watch")]
    pub no_watch: bool,

    /// Skip background mmap warmup after initial scan. Useful for tests.
    #[arg(long = "no-warmup")]
    pub no_warmup: bool,
}

/// Hot-reload the log level. Stub — returns an error until the tracing
/// reload::Layer is wired correctly. Filed as a follow-on task.
pub fn set_log_level(_level: &str) -> Result<(), String> {
    Err("dynamic log level change not yet supported; restart with --log-level <level>".into())
}

#[cfg(unix)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Use the same ~/.cache/ convention as fff-mcp rather than dirs::cache_dir()
    // which returns ~/Library/Caches on macOS — a different path.
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string());
    let default_log = PathBuf::from(format!("{}/.cache/fff_engine.log", home));
    let log_path = args
        .log_file
        .as_deref()
        .and_then(|p| p.to_str())
        .unwrap_or_else(|| default_log.to_str().unwrap_or(""));

    if let Err(e) = fff::log::init_tracing(log_path, Some(&args.log_level)) {
        eprintln!("Warning: failed to init tracing: {e}");
    }

    tracing::info!(
        "fff-engine starting for base_path={}",
        args.base_path.display()
    );

    let socket_path = fff_ipc::socket_path(&args.base_path);
    let lockfile_path = fff_ipc::lockfile_path(&args.base_path);

    let _lockfile_guard = match lifecycle::acquire_lockfile(&lockfile_path) {
        Ok(guard) => guard,
        Err(e) => {
            tracing::info!("Daemon already running (lockfile held): {e}");
            return Ok(());
        }
    };

    let engine_state = Arc::new(state::init(&args).map_err(|e| {
        tracing::error!("State init failed: {e}");
        e
    })?);

    server::run(engine_state, socket_path).await?;

    tracing::info!("fff-engine shutting down");
    Ok(())
}

#[cfg(not(unix))]
fn main() {
    eprintln!("fff-engine is not supported on this platform (Unix only).");
    std::process::exit(1);
}
