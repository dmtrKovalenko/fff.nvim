//! Long-running daemon that indexes directories and serves search requests.
//!
//! The daemon binds a per-user Unix socket at `/tmp/fff-daemon-<uid>.sock`.
//! Clients (typically `fff-rg`) connect and send a single request per
//! connection:
//!
//! 1. Client sends a [`RequestHeader`] + rkyv-serialized [`SearchRequest`]
//!    body, along with an output fd passed via SCM_RIGHTS.
//! 2. Daemon looks up (or creates) a [`FilePicker`] for the requested
//!    directory, runs the search, and writes results directly to the
//!    client's output fd.
//! 3. Daemon writes back a one-byte [`SearchStatus`] and closes the
//!    connection.
//!
//! Directories are indexed on first request and kept alive in a session pool
//! with a background file watcher. Subsequent queries against the same
//! directory reuse the warm index.

mod convert;
pub(crate) mod output;
mod query_service;
mod session_pool;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::Parser;

use crate::query_service::QueryService;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(name = "fff-daemon", about = "FFF file finder daemon")]
struct Args {
    #[arg(long, default_value = "info", env = "FFF_LOG")]
    log_level: String,
}

fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                format!("fff_daemon={lvl},fff={lvl}", lvl = args.log_level).into()
            }),
        )
        .init();

    let shutdown = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, shutdown.clone())
        .expect("failed to register SIGTERM handler");
    signal_hook::flag::register(signal_hook::consts::SIGINT, shutdown.clone())
        .expect("failed to register SIGINT handler");

    QueryService::new(shutdown).run();
}
