mod handlers;
mod lifecycle;
mod server;
mod state;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use clap::Parser;
use mimalloc::MiMalloc;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::{fmt, layer::SubscriberExt, reload, util::SubscriberInitExt, Registry};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Handle for hot-reloading the log filter at runtime via SetLogLevel IPC.
static LOG_LEVEL_HANDLE: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();

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
    /// Can be changed at runtime via `fff-mcp --set-log-level <level>`.
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

/// Update the running daemon's log filter. Called from server.rs on SetLogLevel.
pub fn set_log_level(level: &str) -> Result<(), String> {
    let handle = LOG_LEVEL_HANDLE.get().ok_or("tracing not initialized")?;
    let filter = EnvFilter::new(level);
    handle.reload(filter).map_err(|e| e.to_string())
}

fn setup_tracing(log_file: &Path, log_level: &str) -> Result<WorkerGuard, std::io::Error> {
    use std::fs::OpenOptions;

    if let Some(parent) = log_file.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_file)?;

    let (non_blocking, guard) = tracing_appender::non_blocking(file);

    let filter = EnvFilter::builder()
        .with_default_directive(log_level.parse().unwrap_or_else(|_| {
            tracing::Level::INFO.into()
        }))
        .from_env_lossy();

    let (filter_layer, handle) = reload::Layer::new(filter);
    LOG_LEVEL_HANDLE.set(handle).ok();

    Registry::default()
        .with(filter_layer)
        .with(
            fmt::layer()
                .with_writer(non_blocking)
                .with_target(true)
                .with_ansi(false),
        )
        .init();

    Ok(guard)
}

use std::path::Path;

#[cfg(unix)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let default_log = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("fff_engine.log");
    let log_path = args.log_file.as_deref().unwrap_or(&default_log);

    let _guard = match setup_tracing(log_path, &args.log_level) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Warning: failed to init tracing: {e}");
            // Return a dummy guard so the program continues without logging
            let (_, guard) = tracing_appender::non_blocking(std::io::sink());
            guard
        }
    };

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
