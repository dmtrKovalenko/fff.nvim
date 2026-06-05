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
///
/// Settings are loaded from `$XDG_CONFIG_HOME/fff/config.toml`
/// (or `~/.config/fff/config.toml`). CLI flags override config values.
#[derive(Parser, Debug)]
#[command(name = "fff-engine", version)]
pub(crate) struct Args {
    /// Project root to index. Required.
    #[arg(long = "base-path", value_name = "PATH")]
    pub base_path: PathBuf,

    /// Path to the LMDB frecency database directory.
    /// Overrides config.frecency.db. Default: `~/.local/share/fff/frecency/`.
    #[arg(long = "frecency-db", value_name = "PATH")]
    pub frecency_db_path: Option<PathBuf>,

    /// Path to the log file. Overrides config.log.file.
    #[arg(long = "log-file", value_name = "PATH")]
    pub log_file: Option<PathBuf>,

    /// Log level. Overrides config.log.level.
    #[arg(long = "log-level")]
    pub log_level: Option<String>,

    /// Disable the background filesystem watcher. Overrides config.index.no_watch.
    #[arg(long = "no-watch")]
    pub no_watch: bool,

    /// Skip mmap warmup after initial scan. Overrides config.index.no_warmup.
    #[arg(long = "no-warmup")]
    pub no_warmup: bool,
}

/// Hot-reload the log level. Stub until reload::Layer is wired.
pub fn set_log_level(_level: &str) -> Result<(), String> {
    Err("dynamic log level change requires restart with --log-level <level>".into())
}

#[cfg(unix)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let cfg = fff_ipc::config::load();

    // CLI > config > hardcoded default. Booleans OR together: either source can disable.
    let log_level = args
        .log_level
        .as_deref()
        .unwrap_or(&cfg.log.level)
        .to_string();

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string());
    let default_log = format!("{}/.cache/fff_engine.log", home);
    let log_path = args
        .log_file
        .as_deref()
        .and_then(|p| p.to_str())
        .or(cfg.log.file.as_deref())
        .unwrap_or(&default_log)
        .to_string();

    // fff::log::init_tracing uses EnvFilter::from_env_lossy() which reads RUST_LOG.
    // Set RUST_LOG from our level string so target-based filters like
    // "fff_engine=debug,warn" are honoured. Respect any externally set RUST_LOG.
    if std::env::var("RUST_LOG").is_err() {
        // SAFETY: single-threaded at this point — no other threads exist yet.
        unsafe { std::env::set_var("RUST_LOG", &log_level) };
    }
    if let Err(e) = fff::log::init_tracing(&log_path, Some("info")) {
        eprintln!("Warning: failed to init tracing: {e}");
    }

    tracing::info!(
        "fff-engine starting (config: {}, log: {log_path}, level: {log_level})",
        fff_ipc::config::config_path().display()
    );

    // Effective flags: CLI flag OR config flag (either can disable a feature)
    let eff_no_watch = args.no_watch || cfg.index.no_watch;
    let eff_no_warmup = args.no_warmup || cfg.index.no_warmup;

    // Publish effective settings back into args for state::init to consume.
    // Rebind as a new struct since Args doesn't impl Clone.
    let effective_args = state::EffectiveArgs {
        base_path: args.base_path,
        frecency_db_path: args.frecency_db_path.or_else(|| {
            cfg.frecency.db.as_deref().map(PathBuf::from)
        }),
        no_watch: eff_no_watch,
        no_warmup: eff_no_warmup,
    };

    let socket_path = fff_ipc::socket_path(&effective_args.base_path);
    let lockfile_path = fff_ipc::lockfile_path(&effective_args.base_path);

    let _lockfile_guard = match lifecycle::acquire_lockfile(&lockfile_path) {
        Ok(guard) => guard,
        Err(e) => {
            tracing::info!("Daemon already running (lockfile held): {e}");
            return Ok(());
        }
    };

    let engine_state = Arc::new(state::init(&effective_args).map_err(|e| {
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
