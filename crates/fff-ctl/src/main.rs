//! `fffctl` — operator CLI for fff-engine daemons.
//!
//! Prefers the master management protocol when master is running.
//! Falls back to legacy per-root lockfile scanning when master is absent.

use std::io::{BufReader, BufWriter};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use fff_ipc::lockfile::{self, Lockfile};
use fff_ipc::types::{MasterRequest, MasterResponse};
use fff_ipc::{master_lockfile_path, master_socket_path, read_message_sync, routing_table_path, write_message_sync};

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
    /// List all running daemons (master + workers when master is active).
    List,
    /// Show resolved paths (socket, lockfile, frecency dir, log) for a base-path.
    Paths {
        /// Project root the daemon would (or does) serve.
        #[arg(value_name = "BASE_PATH")]
        base_path: PathBuf,
    },
    /// Query which worker would handle a base-path (read-only).
    Status {
        /// Project root served by the daemon.
        #[arg(value_name = "BASE_PATH")]
        base_path: PathBuf,
    },
    /// Stop daemons. With --all, stops master (which propagates to workers).
    Stop {
        /// Project root served by the daemon. Mutually exclusive with --all.
        #[arg(value_name = "BASE_PATH", conflicts_with = "all")]
        base_path: Option<PathBuf>,
        /// Stop every running daemon (sends SIGTERM to master).
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
    /// Show status of a specific worker by index.
    WorkerStatus {
        /// Worker index.
        #[arg(value_name = "INDEX")]
        index: u32,
    },
    /// List all workers managed by the master.
    ListWorkers,
}

fn main() {
    let cli = Cli::parse();
    let exit = match cli.command {
        Cmd::List => cmd_list(),
        Cmd::ListWorkers => cmd_list_workers(),
        Cmd::Paths { base_path } => cmd_paths(&base_path),
        Cmd::Status { base_path } => cmd_status(&base_path),
        Cmd::WorkerStatus { index } => cmd_worker_status(index),
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
    // Try master management protocol first.
    if let Some(workers) = master_request_list() {
        let master_lock = master_lockfile_path();
        let master_pid = lockfile::read(&master_lock).map(|l| l.pid).unwrap_or(0);
        println!("master PID: {master_pid}  workers: {}", workers.len());
        println!("{:<6}  {:<7}  {:<8}  {}", "INDEX", "PID", "ROOTS", "SOCKET");
        for w in &workers {
            println!("{:<6}  {:<7}  {:<8}  {}", w.index, w.pid, w.root_count(), w.socket_path);
        }
        return 0;
    }

    // Legacy fallback: per-root lockfile scan.
    eprintln!("Note: master not running, showing legacy per-root daemon list");
    let daemons = discover_daemons();
    if daemons.is_empty() {
        println!("No fff-engine daemons running.");
        return 0;
    }
    println!("{:<10}  {:<7}  {:<16}  BASE-PATH", "PID", "STATE", "SLUG");
    for d in &daemons {
        let base = d.lock.base_path.as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        println!("{:<10}  {:<7}  {:<16}  {base}",
            d.lock.pid,
            if d.lock.is_alive() { "live" } else { "stale" },
            d.slug);
    }
    0
}

fn cmd_list_workers() -> i32 {
    match master_request(MasterRequest::ListWorkers) {
        Some(MasterResponse::WorkerList { workers }) => {
            println!("{:<6}  {:<7}  {:<8}  SOCKET", "INDEX", "PID", "ROOTS");
            for w in &workers {
                println!("{:<6}  {:<7}  {:<8}  {}", w.index, w.pid, w.root_count(), w.socket_path);
                for slug in &w.root_slugs {
                    println!("       slug: {slug}");
                }
            }
            0
        }
        Some(MasterResponse::Error(e)) => { eprintln!("master error: {e}"); 1 }
        None => { eprintln!("master not running"); 1 }
        _ => { eprintln!("unexpected response"); 1 }
    }
}

fn cmd_paths(base_path: &Path) -> i32 {
    let slug = fff_ipc::base_path_slug(base_path);
    let socket = fff_ipc::socket_path(base_path);
    let lockfile = fff_ipc::lockfile_path(base_path);
    let frecency = fff_ipc::xdg_data_dir().join("fff").join("frecency").join(&slug);
    let log = fff_ipc::log_path(base_path);
    let master_sock = master_socket_path();
    let master_lock = master_lockfile_path();
    let routing = routing_table_path();

    println!("base_path     : {}", base_path.display());
    println!("slug          : {slug}");
    println!("socket        : {}", socket.display());
    println!("lockfile      : {}", lockfile.display());
    println!("frecency      : {}", frecency.display());
    println!("log           : {}", log.display());
    println!("master.sock   : {}", master_sock.display());
    println!("master.lock   : {}", master_lock.display());
    println!("routing.json  : {}", routing.display());
    0
}

fn cmd_status(base_path: &Path) -> i32 {
    // Use RouteInfo (read-only) when master is running.
    if let Some(resp) = master_request(MasterRequest::RouteInfo { base_path: base_path.to_string_lossy().into() }) {
        match resp {
            MasterResponse::WorkerInfo(info) => {
                println!("Route for {}: worker-{} (pid={}, roots={})",
                    base_path.display(), info.index, info.pid, info.root_count());
                println!("  socket: {}", info.socket_path);
                return 0;
            }
            MasterResponse::Error(e) => {
                println!("{} → {e}", base_path.display());
                return 0;
            }
            _ => {}
        }
    }

    // Legacy fallback.
    let lock_path = fff_ipc::lockfile_path(base_path);
    match lockfile::read(&lock_path) {
        Some(lock) if lock.is_alive() => {
            println!("fff-engine for {} is running (singleton).", base_path.display());
            println!("  PID: {}  lock: {}", lock.pid, lock_path.display());
            0
        }
        Some(lock) => {
            eprintln!("fff-engine for {} is NOT running (stale PID {}).", base_path.display(), lock.pid);
            1
        }
        None => {
            eprintln!("fff-engine for {} is NOT running (no lockfile).", base_path.display());
            1
        }
    }
}

fn cmd_worker_status(index: u32) -> i32 {
    match master_request(MasterRequest::WorkerStatus { index }) {
        Some(MasterResponse::WorkerInfo(info)) => {
            println!("worker-{}: pid={} roots={}", info.index, info.pid, info.root_count());
            println!("  socket: {}", info.socket_path);
            for slug in &info.root_slugs {
                println!("  slug: {slug}");
            }
            0
        }
        Some(MasterResponse::Error(e)) => { eprintln!("master error: {e}"); 1 }
        None => { eprintln!("master not running"); 1 }
        _ => { eprintln!("unexpected response"); 1 }
    }
}

fn cmd_stop(base_path: Option<&Path>, all: bool, timeout: Duration) -> i32 {
    if all {
        // Prefer stopping via master (propagates SIGTERM to all workers).
        let master_lock = master_lockfile_path();
        if let Some(lock) = lockfile::read(&master_lock) {
            if lock.is_alive() {
                let pid = lock.pid as libc::pid_t;
                let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
                if rc == 0 {
                    println!("Sent SIGTERM to master pid={pid}");
                    // Wait for master to exit.
                    let deadline = Instant::now() + timeout;
                    while Instant::now() < deadline && lock.is_alive() {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    return 0;
                }
            }
        }
        // Legacy fallback: stop all per-root daemons.
        let targets: Vec<_> = discover_daemons().into_iter().filter(|d| d.lock.is_alive()).collect();
        if targets.is_empty() {
            println!("No live daemons to stop.");
            return 0;
        }
        let mut failures = 0;
        for d in targets {
            let label = d.lock.base_path.as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| format!("slug={}", d.slug));
            match stop_daemon(&d, timeout) {
                Ok(()) => println!("Stopped PID {} ({label})", d.lock.pid),
                Err(e) => { eprintln!("Failed to stop PID {} ({label}): {e}", d.lock.pid); failures += 1; }
            }
        }
        return if failures > 0 { 1 } else { 0 };
    }

    if let Some(bp) = base_path {
        // Try master StopWorker first.
        let ring_index = master_request(MasterRequest::RouteInfo { base_path: bp.to_string_lossy().into() });
        if let Some(MasterResponse::WorkerInfo(info)) = ring_index {
            if let Some(MasterResponse::Ack) = master_request(MasterRequest::StopWorker { index: info.index }) {
                println!("Stopped worker-{} for {}", info.index, bp.display());
                return 0;
            }
        }

        // Legacy fallback.
        let lock_path = fff_ipc::lockfile_path(bp);
        match lockfile::read(&lock_path) {
            Some(lock) if lock.is_alive() => {
                let d = Daemon { slug: fff_ipc::base_path_slug(bp), lock, lockfile_path: lock_path };
                match stop_daemon(&d, timeout) {
                    Ok(()) => { println!("Stopped PID {}", d.lock.pid); 0 }
                    Err(e) => { eprintln!("Failed: {e}"); 1 }
                }
            }
            _ => { eprintln!("No live daemon for {}", bp.display()); 1 }
        }
    } else {
        eprintln!("Specify a base-path or pass --all.");
        2
    }
}

fn cmd_clean(dry_run: bool) -> i32 {
    let mut removed_master = 0usize;
    let mut removed_locks = 0;
    let mut removed_sockets = 0;
    let mut removed_logs = 0;

    // ── Master + worker artifacts ─────────────────────────────────────
    let master_lock = master_lockfile_path();
    if lockfile::read(&master_lock).is_some_and(|l| l.is_alive()) {
        println!("Note: master is running; skipping master artifacts (use `fffctl stop --all` first).");
    } else {
        removed_master = clean_master_artifacts(dry_run);
    }

    // ── Legacy per-root artifacts ─────────────────────────────────────
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
        "{}: {} master artifact(s), {} lockfile(s), {} socket(s), {} log(s)",
        if dry_run { "Would remove" } else { "Removed" },
        removed_master,
        removed_locks,
        removed_sockets,
        removed_logs,
    );
    0
}

fn clean_master_artifacts(dry_run: bool) -> usize {
    let mut removed = 0;
    let action = if dry_run { "would remove" } else { "removing" };

    let routing = routing_table_path();
    if routing.exists() {
        println!("{action} routing table: {}", routing.display());
        if !dry_run { let _ = std::fs::remove_file(&routing); }
        removed += 1;
    }

    let master_sock = master_socket_path();
    if master_sock.exists() {
        println!("{action} master socket: {}", master_sock.display());
        if !dry_run { let _ = std::fs::remove_file(&master_sock); }
        removed += 1;
    }

    let master_lock = master_lockfile_path();
    if master_lock.exists() {
        println!("{action} master lockfile: {}", master_lock.display());
        if !dry_run { let _ = std::fs::remove_file(&master_lock); }
        removed += 1;
    }

    let workers_dir = fff_ipc::xdg_cache_dir().join("fff").join("workers");
    if let Ok(entries) = std::fs::read_dir(&workers_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|s| s.to_str()) else { continue };
            if ext == "sock" || ext == "lock" {
                println!("{action} worker artifact: {}", path.display());
                if !dry_run { let _ = std::fs::remove_file(&path); }
                removed += 1;
            }
        }
    }

    removed
}

// ─────────────────────────────────────────────────────────────────────────────
// Master management helpers

/// Send one request to the master socket and return the response, or None if master is unreachable.
fn master_request(req: MasterRequest) -> Option<MasterResponse> {
    let socket = master_socket_path();
    let stream = UnixStream::connect(&socket).ok()?;
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

    let mut writer = BufWriter::new(stream.try_clone().ok()?);
    let mut reader = BufReader::new(stream);

    write_message_sync(&mut writer, &req).ok()?;
    use std::io::Write;
    writer.flush().ok()?;
    let resp: MasterResponse = read_message_sync(&mut reader).ok()?;
    Some(resp)
}

/// Convenience: list workers via master. Returns None if master is not running.
fn master_request_list() -> Option<Vec<fff_ipc::types::WorkerInfo>> {
    match master_request(MasterRequest::ListWorkers)? {
        MasterResponse::WorkerList { workers } => Some(workers),
        _ => None,
    }
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
