mod handlers;
mod lifecycle;
pub(crate) mod master;
pub(crate) mod ring;
mod server;
mod state;
pub(crate) mod worker;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// fff search engine daemon — singleton, master, or worker mode.
#[derive(Parser, Debug)]
#[command(name = "fff-engine", version)]
pub(crate) struct Args {
    /// Run as the master router process (spawns and routes to workers).
    #[arg(long = "master", conflicts_with_all = ["worker_index", "base_path"])]
    pub master: bool,

    /// Run as a worker process with the given index.
    #[arg(long = "worker-index", value_name = "N", conflicts_with = "base_path")]
    pub worker_index: Option<u32>,

    /// Project root to index (singleton mode only). Required in singleton mode.
    #[arg(long = "base-path", value_name = "PATH")]
    pub base_path: Option<PathBuf>,

    /// Path to the LMDB frecency database directory. Overrides config.
    #[arg(long = "frecency-db", value_name = "PATH")]
    pub frecency_db_path: Option<PathBuf>,

    /// Path to the log file. Overrides config.log.file.
    #[arg(long = "log-file", value_name = "PATH")]
    pub log_file: Option<PathBuf>,

    /// Log level. Overrides config.log.level.
    #[arg(long = "log-level")]
    pub log_level: Option<String>,

    /// Disable the background filesystem watcher.
    #[arg(long = "no-watch")]
    pub no_watch: bool,

    /// Skip mmap warmup after initial scan.
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

    let log_level = args
        .log_level
        .as_deref()
        .unwrap_or(&cfg.log.level)
        .to_string();

    if std::env::var("RUST_LOG").is_err() {
        // SAFETY: single-threaded at this point — no other threads exist yet.
        unsafe { std::env::set_var("RUST_LOG", &log_level) };
    }

    // ── Master mode ───────────────────────────────────────────────────────────
    if args.master {
        let log_path_str = cfg.log.file.clone().unwrap_or_else(|| {
            fff_ipc::xdg_cache_dir().join("fff").join("master.log").to_string_lossy().into()
        });
        if let Err(e) = fff::log::init_tracing(&log_path_str, Some("info")) {
            eprintln!("Warning: failed to init tracing: {e}");
        }
        tracing::info!("fff-engine master starting");
        return master::run(cfg).await;
    }

    // ── Worker mode ───────────────────────────────────────────────────────────
    if let Some(index) = args.worker_index {
        let log_path_str = cfg.log.file.clone().unwrap_or_else(|| {
            fff_ipc::xdg_cache_dir()
                .join("fff")
                .join(format!("worker-{index}.log"))
                .to_string_lossy()
                .into()
        });
        if let Err(e) = fff::log::init_tracing(&log_path_str, Some("info")) {
            eprintln!("Warning: failed to init tracing: {e}");
        }
        tracing::info!("fff-engine worker-{index} starting");
        return worker::run(index, cfg).await;
    }

    // ── Singleton mode (legacy / direct invocation) ───────────────────────────
    let base_path = args.base_path.ok_or("--base-path is required in singleton mode")?;

    let default_log = fff_ipc::log_path(&base_path);
    let default_log_str = default_log.to_string_lossy().into_owned();
    let log_path = args
        .log_file
        .as_deref()
        .and_then(|p| p.to_str())
        .or(cfg.log.file.as_deref())
        .unwrap_or(&default_log_str)
        .to_string();

    if let Err(e) = fff::log::init_tracing(&log_path, Some("info")) {
        eprintln!("Warning: failed to init tracing: {e}");
    }

    tracing::info!("fff-engine singleton starting (level: {log_level})");

    let eff_no_watch = args.no_watch || cfg.index.no_watch;
    let eff_no_warmup = args.no_warmup || cfg.index.no_warmup;

    let effective_args = state::EffectiveArgs {
        base_path: base_path.clone(),
        frecency_db_path: args.frecency_db_path.or_else(|| {
            cfg.frecency.db.as_deref().map(PathBuf::from)
        }),
        no_watch: eff_no_watch,
        no_warmup: eff_no_warmup,
    };

    let socket_path = fff_ipc::socket_path(&base_path);
    let lockfile_path = fff_ipc::lockfile_path(&base_path);

    let _lockfile_guard = match lifecycle::acquire_lockfile(&lockfile_path, &base_path) {
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

    tracing::info!("fff-engine singleton shutting down");
    Ok(())
}

#[cfg(not(unix))]
fn main() {
    eprintln!("fff-engine is not supported on this platform (Unix only).");
    std::process::exit(1);
}
