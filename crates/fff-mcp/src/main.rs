//! FFF MCP Server — high-performance file finder for AI code assistants.
//!
//! Drop-in replacement for AI code assistant file search tools (Glob/Grep).
//! Provides frecency-ranked, fuzzy-matched, git-aware file finding and
//! code search via the Model Context Protocol (MCP).
//!
//! Uses `fff-core` directly (zero FFI overhead) for all search operations.

mod cursor;
mod output;
mod server;
mod update_check;

use std::sync::{Arc, RwLock};

use clap::Parser;
use fff_core::file_picker::FilePicker;
use fff_core::frecency::FrecencyTracker;
use fff_core::{FFFMode, SharedFrecency, SharedPicker};
use git2::Repository;
use mimalloc::MiMalloc;
use rmcp::{ServiceExt, transport::stdio};
use server::FffServer;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub const MCP_INSTRUCTIONS: &str = concat!(
    "FFF is a fast file finder with frecency-ranked results (frequent/recent files first, git-dirty files boosted).\n",
    "\n",
    "## Which Tool Should I Use?\n",
    "\n",
    "- **grep**: DEFAULT tool. Searches file CONTENTS -- definitions, usage, patterns. Use when you have a specific name or pattern.\n",
    "- **find_files**: Explores which files/modules exist for a topic. Use when you DON'T have a specific identifier or LOOKING FOR A FILE.\n",
    "- **multi_grep**: OR logic across multiple patterns. Use for case variants (e.g. ['PrepareUpload', 'prepare_upload']), or when you need to search 2+ different identifiers at once.\n",
    "\n",
    "## Core Rules\n",
    "\n",
    "### 1. Search BARE IDENTIFIERS only\n",
    "Grep matches single lines. Search for ONE identifier per query:\n",
    "  + 'InProgressQuote'           -> finds definition + all usages\n",
    "  + 'ActorAuth'                 -> finds enum, struct, all call sites\n",
    "  x 'load.*metadata.*InProgressQuote' -> regex spanning multiple tokens, 0 results\n",
    "  x 'ctx.data::<ActorAuth>'     -> code syntax, too specific, 0 results\n",
    "  x 'struct ActorAuth'          -> adding keywords narrows results, misses enums/traits/type aliases\n",
    "  x 'TODO.*#\\d+'               -> complex regex, use simple 'TODO' then filter visually\n",
    "\n",
    "### 2. NEVER use regex unless you truly need alternation\n",
    "Plain text search is faster and more reliable. Regex patterns like `.*`, `\\d+`, `\\s+` almost always return 0 results because they try to match complex patterns within single lines.\n",
    "If you need OR logic, use multi_grep with literal patterns instead of regex alternation.\n",
    "\n",
    "### 3. Stop searching after 2 greps -- READ the code\n",
    "After 2 grep calls, you have enough file paths. Read the top result to understand the code.\n",
    "Do NOT keep grepping with variations. More greps != better understanding.\n",
    "\n",
    "### 4. Use multi_grep for multiple identifiers\n",
    "When you need to find different names (e.g. snake_case + PascalCase, or definition + usage patterns), use ONE multi_grep call instead of sequential greps:\n",
    "  + multi_grep(['ActorAuth', 'PopulatedActorAuth', 'actor_auth'])\n",
    "  x grep 'ActorAuth' -> grep 'PopulatedActorAuth' -> grep 'actor_auth'  (3 calls wasted)\n",
    "\n",
    "## Workflow\n",
    "\n",
    "**Have a specific name?** -> grep the bare identifier.\n",
    "**Need multiple name variants?** -> multi_grep with all variants in one call.\n",
    "**Exploring a topic / finding files?** -> find_files.\n",
    "**Got results?** -> Read the top file. Don't grep again.\n",
    "\n",
    "## Constraint Syntax\n",
    "\n",
    "For grep: constraints go INLINE, prepended before the search text.\n",
    "For multi_grep: constraints go in the separate 'constraints' parameter.\n",
    "\n",
    "Constraints MUST match one of these formats:\n",
    "  Extension: '*.rs', '*.{ts,tsx}'  (starts with *.)\n",
    "  Directory: 'src/', 'quotes/'     (ends with /)\n",
    "  Exclude:   '!test/', '!*.spec.ts' (starts with !)\n",
    "\n",
    "! Bare words are NOT constraints. 'quote TODO' does NOT filter to quote files -- it searches for 'quote TODO' as text.\n",
    "  + 'quotes/ TODO'   -> searches for 'TODO' in the quotes/ directory\n",
    "  x 'quote TODO'     -> searches for literal text 'quote TODO', finds nothing\n",
    "\n",
    "Prefer broad constraints:\n",
    "  + '*.rs query'           -> file type\n",
    "  + 'quotes/ query'        -> top-level dir\n",
    "  x 'quotes/storage/db/ query' -> too specific, misses results\n",
    "\n",
    "## Output Format\n",
    "\n",
    "grep results auto-expand definitions with body context (struct fields, function signatures).\n",
    "This often provides enough information WITHOUT a follow-up Read call.\n",
    "Lines marked with | are definition body context. [def] marks definition files.\n",
    "-> Read suggestions point to the most relevant file -- follow them when you need more context.\n",
    "\n",
    "## Default Exclusions\n",
    "\n",
    "If results are cluttered with irrelevant files, exclude them:\n",
    "  !tests/ - exclude tests directory\n",
    "  !*.spec.ts - exclude test files\n",
    "  !generated/ - exclude generated code",
);

/// FFF MCP Server — high-performance file finder for AI code assistants.
#[derive(Parser)]
#[command(name = "fff-mcp", version = env!("CARGO_PKG_VERSION"))]
struct Args {
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

    /// Path to the log file.
    #[arg(long = "log-file")]
    log_file: Option<String>,

    /// Log level (e.g. trace, debug, info, warn, error).
    #[arg(long = "log-level")]
    log_level: Option<String>,

    /// Disable automatic update checks on startup.
    #[arg(long = "no-update-check")]
    no_update_check: bool,
}

/// Resolve default paths for frecency db, history db, and log file.
/// Shares Neovim's standard data locations when they exist so the MCP
/// server and fff.nvim plugin use the same databases.
fn resolve_defaults(args: &mut Args) {
    let home = dirs_home();
    let is_windows = cfg!(target_os = "windows");

    let nvim_cache_dir = if is_windows {
        format!("{}\\AppData\\Local\\nvim-data", home)
    } else {
        format!("{}/.cache/nvim", home)
    };
    let nvim_data_dir = if is_windows {
        format!("{}\\AppData\\Local\\nvim-data", home)
    } else {
        format!("{}/.local/share/nvim", home)
    };

    let use_nvim_paths = std::path::Path::new(&nvim_cache_dir).exists()
        || std::path::Path::new(&nvim_data_dir).exists();

    if args.frecency_db_path.is_none() {
        args.frecency_db_path = Some(if use_nvim_paths {
            format!("{}/fff_nvim", nvim_cache_dir)
        } else {
            format!("{}/.fff/frecency.mdb", home)
        });
    }
    if args.history_db_path.is_none() {
        args.history_db_path = Some(if use_nvim_paths {
            format!("{}/fff_queries", nvim_data_dir)
        } else {
            format!("{}/.fff/history.mdb", home)
        });
    }

    // Ensure parent directories exist for database paths
    for path in [&args.frecency_db_path, &args.history_db_path]
        .into_iter()
        .flatten()
    {
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    if args.log_file.is_none() {
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

    let log_file = args.log_file.as_deref().unwrap_or("");
    if let Err(e) = fff_core::log::init_tracing(log_file, args.log_level.as_deref()) {
        eprintln!("Warning: Failed to init tracing: {}", e);
    }

    let base_path = args.base_path.unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    });

    if !Repository::discover(&base_path).is_ok() {
        tracing::error!("MCP server must be run within a Git repository");
        return Err(format!("Not a Git repository: {}", base_path).into());
    }

    let frecency_db_path = args.frecency_db_path.unwrap_or_default();

    let shared_picker: SharedPicker = Arc::new(RwLock::new(None));
    let shared_frecency: SharedFrecency = Arc::new(RwLock::new(None));
    match FrecencyTracker::new(&frecency_db_path, false) {
        Ok(tracker) => {
            if let Ok(mut guard) = shared_frecency.write() {
                *guard = Some(tracker);
            }
            FrecencyTracker::spawn_gc(Arc::clone(&shared_frecency), frecency_db_path, false);
        }
        Err(e) => {
            eprintln!("Warning: Failed to init frecency db: {}", e);
        }
    }

    // Initialize file picker (spawns background scan + watcher)
    FilePicker::new_with_shared_state(
        base_path,
        true, // warmup_mmap_cache
        FFFMode::Ai,
        Arc::clone(&shared_picker),
        Arc::clone(&shared_frecency),
    )
    .map_err(|e| format!("Failed to init file picker: {}", e))?;

    if !args.no_update_check {
        update_check::spawn_update_check();
    }

    // Create and start the MCP server
    let server = FffServer::new(shared_picker.clone(), shared_frecency.clone());

    // Wait for initial scan in background — don't block server startup
    let picker_clone_for_scan = Arc::clone(&shared_picker);
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

    let service = server
        .serve(stdio())
        .await
        .map_err(|e| format!("Failed to start MCP server: {}", e))?;

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
