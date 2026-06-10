//! `fff-rg` — drop-in `rg`-compatible CLI that accelerates searches inside
//! git repositories by delegating to [`fff-daemon`] over a Unix socket.
//!
//! On startup it probes the working directory for a git worktree. If one is
//! found, requests are serialized via rkyv and sent to the daemon (spawning
//! it on first use). The daemon keeps a warm file index, so repeated queries
//! skip the filesystem walk entirely. Outside a git worktree the tool falls
//! back to a plain `rg` subprocess, behaving identically to upstream ripgrep.
//!
//! Exit codes follow `rg` conventions: 0 = match, 1 = no match, 2 = error.

mod app_ctx;
mod searcher;
mod types;

use clap::Parser;

use crate::app_ctx::AppCtx;
use crate::searcher::{Search, Searcher};
use crate::types::cli::Args;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(
            |_| format!("fff_rg={lvl},fff_ipc_domain={lvl}", lvl = args.log_level).into(),
        ))
        .init();

    if !args.files && args.pattern.is_none() {
        eprintln!("error: PATTERN is required (use --files to list files)");
        std::process::exit(2);
    }

    let searcher = Searcher::new(AppCtx::new(&args));

    let found = if args.files { searcher.files()? } else { searcher.grep()? };

    if !found {
        std::process::exit(1);
    }
    Ok(())
}
