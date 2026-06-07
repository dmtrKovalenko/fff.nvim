//! `fffctl` — operator CLI for fff-engine daemons.
//!
//! Each daemon is identified by a stable hash of its canonical base-path
//! ([`fff_ipc::base_path_slug`]). fffctl discovers daemons by scanning the
//! lockfile directory (`$XDG_CACHE_HOME/fff/locks/`) and parsing the
//! `<pid>\n<base-path>` payload that fff-engine writes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use fff_ipc::lockfile::{self, Lockfile};

#[derive(Parser, Debug)]
#[command(
    name = "fffctl",
    version,
    about = "Manage fff-engine daemons",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// List all running daemons.
    List,
    /// Show resolved paths (socket, lockfile, frecency dir, log) for a base-path.
    Paths {
        /// Project root the daemon would (or does) serve.
        #[arg(value_name = "BASE_PATH")]
        base_path: PathBuf,
    },
    /// Run a healthcheck against a running daemon.
    Status {
        /// Project root served by the daemon.
        #[arg(value_name = "BASE_PATH")]
        base_path: PathBuf,
    },
    /// Stop a running daemon by sending SIGTERM (then SIGKILL after timeout).
    Stop {
        /// Project root served by the daemon. Mutually exclusive with --all.
        #[arg(value_name = "BASE_PATH", conflicts_with = "all")]
        base_path: Option<PathBuf>,
        /// Stop every running daemon.
        #[arg(long, conflicts_with = "base_path")]
        all: bool,
        /// Seconds to wait for graceful exit before SIGKILL. 0 disables KILL.
        #[arg(long, default_value_t = 5)]
        timeout: u64,
    },
    /// Remove stale lockfiles, orphan sockets, and unreferenced frecency dirs.
    Clean {
        /// Print actions without performing them.
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let exit = match cli.command {
        Cmd::List => cmd_list(),
        Cmd::Paths { base_path } => cmd_paths(&base_path),
        Cmd::Status { base_path } => cmd_status(&base_path),
        Cmd::Stop {
            base_path,
            all,
            timeout,
        } => cmd_stop(base_path.as_deref(), all, Duration::from_secs(timeout)),
        Cmd::Clean { dry_run } => cmd_clean(dry_run),
    };
    std::process::exit(exit);
}

// ─────────────────────────────────────────────────────────────────────────────
// Commands

fn cmd_list() -> i32 {
    let daemons = discover_daemons();
    if daemons.is_empty() {
        println!("No fff-engine daemons running.");
        return 0;
    }

    println!("{:<10}  {:<7}  {:<16}  BASE-PATH", "PID", "STATE", "SLUG");
    for d in &daemons {
        let base = d
            .lock
            .base_path
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unknown — pre-0.11 lockfile>".to_string());
        println!(
            "{:<10}  {:<7}  {:<16}  {base}",
            d.lock.pid,
            if d.lock.is_alive() { "live" } else { "stale" },
            d.slug,
        );
    }
    0
}

fn cmd_paths(base_path: &Path) -> i32 {
    let slug = fff_ipc::base_path_slug(base_path);
    let socket = fff_ipc::socket_path(base_path);
    let lockfile = fff_ipc::lockfile_path(base_path);
    let frecency = fff_ipc::xdg_data_dir()
        .join("fff")
        .join("frecency")
        .join(&slug);
    let log = fff_ipc::log_path(base_path);

    println!("base_path : {}", base_path.display());
    println!("slug      : {slug}");
    println!("socket    : {}", socket.display());
    println!("lockfile  : {}", lockfile.display());
    println!("frecency  : {}", frecency.display());
    println!("log       : {}", log.display());
    0
}

fn cmd_status(base_path: &Path) -> i32 {
    let lock_path = fff_ipc::lockfile_path(base_path);
    match lockfile::read(&lock_path) {
        Some(lock) if lock.is_alive() => {
            println!("fff-engine for {} is running.", base_path.display());
            println!("  PID  : {}", lock.pid);
            println!("  lock : {}", lock_path.display());
            0
        }
        Some(lock) => {
            eprintln!(
                "fff-engine for {} is NOT running (stale lockfile, PID {}).",
                base_path.display(),
                lock.pid
            );
            1
        }
        None => {
            eprintln!(
                "fff-engine for {} is NOT running (no lockfile at {}).",
                base_path.display(),
                lock_path.display()
            );
            1
        }
    }
}

fn cmd_stop(base_path: Option<&Path>, all: bool, timeout: Duration) -> i32 {
    let targets: Vec<Daemon> = if all {
        discover_daemons().into_iter().filter(|d| d.lock.is_alive()).collect()
    } else if let Some(bp) = base_path {
        let lock_path = fff_ipc::lockfile_path(bp);
        match lockfile::read(&lock_path) {
            Some(lock) if lock.is_alive() => vec![Daemon {
                slug: fff_ipc::base_path_slug(bp),
                lock,
                lockfile_path: lock_path,
            }],
            _ => {
                eprintln!("No live daemon for {}", bp.display());
                return 1;
            }
        }
    } else {
        eprintln!("Specify a base-path or pass --all.");
        return 2;
    };

    if targets.is_empty() {
        println!("No live daemons to stop.");
        return 0;
    }

    let mut failures = 0;
    for d in targets {
        let label = d
            .lock
            .base_path
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| format!("slug={}", d.slug));
        match stop_daemon(&d, timeout) {
            Ok(()) => println!("Stopped PID {} ({label})", d.lock.pid),
            Err(e) => {
                eprintln!("Failed to stop PID {} ({label}): {e}", d.lock.pid);
                failures += 1;
            }
        }
    }
    if failures > 0 { 1 } else { 0 }
}

fn cmd_clean(dry_run: bool) -> i32 {
    let mut removed_locks = 0;
    let mut removed_sockets = 0;
    let mut removed_logs = 0;

    // Stale lockfiles
    for d in discover_daemons() {
        if d.lock.is_alive() {
            continue;
        }
        let action = if dry_run { "would remove" } else { "removing" };
        println!(
            "{action} stale lockfile: {} (PID {} dead)",
            d.lockfile_path.display(),
            d.lock.pid
        );
        if !dry_run {
            let _ = std::fs::remove_file(&d.lockfile_path);
        }
        removed_locks += 1;
    }

    let cache = fff_ipc::xdg_cache_dir().join("fff");
    let lock_dir = cache.join("locks");

    // Orphan sockets (no matching live lockfile)
    let socket_dir = cache.join("sockets");
    if let Ok(entries) = std::fs::read_dir(&socket_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("sock") {
                continue;
            }
            let Some(slug) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let lock = lock_dir.join(format!("{slug}.lock"));
            if lockfile::read(&lock).is_some_and(|l| l.is_alive()) {
                continue;
            }
            let action = if dry_run { "would remove" } else { "removing" };
            println!("{action} orphan socket: {}", path.display());
            if !dry_run {
                let _ = std::fs::remove_file(&path);
            }
            removed_sockets += 1;
        }
    }

    // Orphan log files (no matching live lockfile)
    let log_dir = cache.join("logs");
    if let Ok(entries) = std::fs::read_dir(&log_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("log") {
                continue;
            }
            let Some(slug) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let lock = lock_dir.join(format!("{slug}.lock"));
            if lockfile::read(&lock).is_some_and(|l| l.is_alive()) {
                continue;
            }
            let action = if dry_run { "would remove" } else { "removing" };
            println!("{action} orphan log: {}", path.display());
            if !dry_run {
                let _ = std::fs::remove_file(&path);
            }
            removed_logs += 1;
        }
    }

    println!(
        "{}: {} lockfile(s), {} socket(s), {} log(s)",
        if dry_run { "Would remove" } else { "Removed" },
        removed_locks,
        removed_sockets,
        removed_logs,
    );
    0
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers

struct Daemon {
    slug: String,
    lock: Lockfile,
    lockfile_path: PathBuf,
}

fn discover_daemons() -> Vec<Daemon> {
    let lock_dir = fff_ipc::xdg_cache_dir().join("fff").join("locks");
    let Ok(entries) = std::fs::read_dir(&lock_dir) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("lock") {
            continue;
        }
        let Some(slug) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Some(lock) = lockfile::read(&path) {
            out.push(Daemon {
                slug: slug.to_string(),
                lock,
                lockfile_path: path,
            });
        }
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    out
}

fn stop_daemon(d: &Daemon, timeout: Duration) -> Result<(), String> {
    let pid = d.lock.pid as libc::pid_t;
    // SAFETY: SIGTERM to a known PID. errno on failure is surfaced via the
    // standard errno() route below.
    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(format!("SIGTERM failed: {err}"));
    }

    let deadline = Instant::now() + timeout;
    let poll = Duration::from_millis(50);
    while Instant::now() < deadline {
        if !d.lock.is_alive() {
            return Ok(());
        }
        std::thread::sleep(poll);
    }

    if timeout.is_zero() {
        return Err("did not exit; --timeout 0 disables SIGKILL".into());
    }
    // SAFETY: SIGKILL after the graceful window elapsed.
    let rc = unsafe { libc::kill(pid, libc::SIGKILL) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(format!("SIGKILL failed: {err}"));
    }
    Ok(())
}
