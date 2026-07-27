mod cursor;
mod healthcheck;
mod instructions;
mod output;
mod server;
mod update_check;

use clap::{Parser, ValueEnum};
use fff::file_picker::FilePicker;
use fff::frecency::FrecencyTracker;
use fff::{FFFMode, SharedFilePicker, SharedFrecency};
use git2::Repository;
use mimalloc::MiMalloc;
use rmcp::{ServiceExt, transport::stdio};
use server::FffServer;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub use instructions::build_instructions;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum ExposedTool {
    FindFiles,
    Grep,
    MultiGrep,
}

impl ExposedTool {
    pub fn tool_name(self) -> &'static str {
        match self {
            ExposedTool::FindFiles => "find_files",
            ExposedTool::Grep => "grep",
            ExposedTool::MultiGrep => "multi_grep",
        }
    }

    pub fn all() -> [ExposedTool; 3] {
        [
            ExposedTool::FindFiles,
            ExposedTool::Grep,
            ExposedTool::MultiGrep,
        ]
    }
}

/// FFF MCP Server -- a high performance & accuracy file finder for AI code assistants.
#[derive(Parser)]
#[command(name = "fff-mcp", version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("FFF_GIT_HASH"), ")"))]
pub(crate) struct Args {
    /// Base directory to index. Defaults to the current working directory.
    #[arg(value_name = "PATH")]
    base_path: Option<String>,

    /// Path to the frecency database.
    #[arg(long = "frecency-db")]
    frecency_db_path: Option<String>,

    /// Path to the query history database.
    #[arg(long = "history-db")]
    #[allow(dead_code)]
    history_db_path: Option<String>,

    /// Path-shape hint for per-session log files.
    /// Each fff-mcp startup writes a fresh sibling file `<stem>+<UTC-timestamp>+<pid>.<ext>`
    #[arg(long = "log-file")]
    log_file: Option<String>,

    /// Log level (e.g. trace, debug, info, warn, error).
    #[arg(long = "log-level")]
    log_level: Option<String>,

    /// Disable automatic update checks on startup.
    #[arg(long = "no-update-check")]
    no_update_check: bool,

    /// Disable eager mmap warmup after the initial scan. Grep results will
    /// still work (files are mmap'd lazily on first access), but the first
    /// search may be slightly slower. Useful on very large repos where the
    /// warmup would consume too many kernel resources.
    #[arg(long = "no-warmup")]
    no_warmup: bool,

    /// Disable the content index built after the initial scan.
    /// This makes grep calls slower but consumes less RAM (recommended to not turn off)
    no_content_indexing: bool,

    /// Explicitly enable content indexing even when `--no-warmup` is set.
    #[arg(long = "content-indexing")]
    content_indexing: bool,

    /// Disable the background file-system watcher. Files are scanned once
    /// at startup but not monitored for changes.
    #[arg(long = "no-watch")]
    no_watch: bool,

    /// Maximum number of files whose content is kept persistently in memory.
    /// Files beyond this limit are still searchable via temporary mmaps that
    /// are released after each grep. Defaults to 30 000.
    /// Also settable via the FFF_MAX_CACHED_FILES environment variable.
    #[arg(long = "max-cached-files", env = "FFF_MAX_CACHED_FILES")]
    max_cached_files: Option<usize>,

    /// Follow symlinks during scan and watcher walks. Off by default —
    /// enabling on cyclic symlink layouts can wedge the watcher.
    #[arg(long = "follow-symlinks")]
    follow_symlinks: bool,

    /// Run a health check and print diagnostic information, then exit.
    #[arg(long = "healthcheck")]
    pub(crate) healthcheck: bool,

    /// Exit after this many seconds of inactivity. 0 = never exit.
    #[arg(
        long = "idle-timeout-secs",
        env = "FFF_MCP_IDLE_TIMEOUT_SECS",
        default_value_t = 900
    )]
    idle_timeout_secs: u64,

    /// Comma-separated list of tools to expose. Defaults to all three.
    /// Unknown names cause startup to fail with the list of valid values.
    #[arg(
        long = "tools",
        value_enum,
        value_delimiter = ',',
        num_args = 1..,
    )]
    tools: Option<Vec<ExposedTool>>,
}

/// Resolve default paths for the log file.
/// Database paths (frecency, history) must be explicitly provided via flags.
fn resolve_defaults(args: &mut Args) {
    // Ensure parent directories exist for database paths when provided
    for path in [&args.frecency_db_path, &args.history_db_path]
        .into_iter()
        .flatten()
    {
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    if args.log_file.is_none() {
        let home = dirs_home();
        let is_windows = cfg!(target_os = "windows");
        args.log_file = Some(if is_windows {
            format!("{}\\AppData\\Local\\fff_mcp.log", home)
        } else {
            format!("{}/.cache/fff_mcp.log", home)
        });
    }
}

fn dirs_home() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::parse();
    resolve_defaults(&mut args);

    if args.healthcheck {
        return healthcheck::run_healthcheck(&args);
    }

    let log_file = args.log_file.as_deref().unwrap_or("");
    if let Err(e) = fff::log::init_tracing(log_file, args.log_level.as_deref(), None) {
        eprintln!("Warning: Failed to init tracing: {}", e);
    }

    let base_path = args.base_path.unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    });

    let base_path = match Repository::discover(&base_path) {
        Ok(repo) => {
            if let Some(workdir) = repo.workdir() {
                let git_root = workdir.to_string_lossy().to_string();
                tracing::info!("Discovered git root: {}", git_root);
                git_root
            } else {
                tracing::info!("Git repository is bare, using base path: {}", base_path);
                base_path
            }
        }
        Err(_) => {
            tracing::info!(
                "No git repository found, indexing from base path: {}",
                base_path
            );
            base_path
        }
    };

    let shared_picker = SharedFilePicker::default();
    let shared_frecency = SharedFrecency::default();
    if let Some(frecency_db_path) = args.frecency_db_path {
        match FrecencyTracker::open(&frecency_db_path) {
            Ok(tracker) => {
                let _ = shared_frecency.init(tracker);
            }
            Err(e) => {
                eprintln!("Warning: Failed to init frecency db: {}", e);
            }
        }
    }

    // Content indexing follows warmup by default (backward compat), unless
    // the user explicitly opts in via --content-indexing or out via
    // --no-content-indexing.
    let enable_content_indexing = if args.content_indexing {
        true
    } else if args.no_content_indexing {
        false
    } else {
        !args.no_warmup
    };

    // Initialize file picker (spawns background scan + watcher)
    FilePicker::new_with_shared_state(
        shared_picker.clone(),
        shared_frecency,
        fff::FilePickerOptions {
            base_path,
            enable_mmap_cache: !args.no_warmup,
            enable_content_indexing,
            watch: !args.no_watch,
            mode: FFFMode::Ai,
            cache_budget: args
                .max_cached_files
                .map(fff::ContentCacheBudget::new_for_repo),
            follow_symlinks: args.follow_symlinks,
            ..Default::default()
        },
    )
    .map_err(|e| format!("Failed to init file picker: {}", e))?;

    if !args.no_update_check {
        update_check::spawn_update_check();
    }

    let exposed_tools: Vec<ExposedTool> = args
        .tools
        .clone()
        .map(|mut v| {
            v.sort_by_key(|t| *t as u8);
            v.dedup();
            v
        })
        .unwrap_or_else(|| ExposedTool::all().to_vec());

    // Create and start the MCP server
    let server = FffServer::new(shared_picker.clone(), &exposed_tools);
    let last_activity = server.last_activity();
    let idle_timeout_secs = args.idle_timeout_secs;

    // Wait for initial scan in background — don't block server startup
    let picker_clone_for_scan = shared_picker.clone();
    tokio::task::spawn_blocking(move || {
        let start = std::time::Instant::now();
        loop {
            let is_scanning = picker_clone_for_scan
                .read()
                .ok()
                .and_then(|g| g.as_ref().map(|p| p.is_scan_active()))
                .unwrap_or(true);

            if !is_scanning {
                tracing::info!("Initial scan completed in {:?}", start.elapsed());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });

    const STARTUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
    let service = match tokio::time::timeout(STARTUP_TIMEOUT, server.serve(stdio())).await {
        Ok(res) => res.map_err(|e| format!("Failed to start MCP server: {}", e))?,
        Err(_) => {
            return Err("MCP initialize handshake did not complete within 60s".into());
        }
    };

    if idle_timeout_secs > 0 {
        last_activity.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            std::sync::atomic::Ordering::Relaxed,
        );

        let last_activity_for_watchdog = last_activity.clone();
        tokio::spawn(async move {
            let tick = std::time::Duration::from_secs(60);
            loop {
                tokio::time::sleep(tick).await;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                let last = last_activity_for_watchdog.load(std::sync::atomic::Ordering::Relaxed);
                if now.saturating_sub(last) >= idle_timeout_secs {
                    tracing::info!(
                        "Exiting after {}s of inactivity (idle_timeout_secs={})",
                        now.saturating_sub(last),
                        idle_timeout_secs
                    );
                    std::process::exit(0);
                }
            }
        });
    }

    let picker_for_shutdown = shared_picker.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        if let Ok(mut guard) = picker_for_shutdown.write()
            && let Some(ref mut picker) = *guard
        {
            picker.stop_background_monitor();
        }
        std::process::exit(0);
    });

    service.waiting().await?;

    if let Ok(mut guard) = shared_picker.write()
        && let Some(ref mut picker) = *guard
    {
        picker.stop_background_monitor();
    }

    Ok(())
}
