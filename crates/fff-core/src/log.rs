//! Shared logging utilities for FFF crates.
//!
//! Provides file-based tracing initialization and a panic hook that writes
//! to both stderr and a fallback log file.

use std::io;
use std::path::Path;
use tracing_appender::non_blocking;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

static TRACING_INITIALIZED: std::sync::OnceLock<tracing_appender::non_blocking::WorkerGuard> =
    std::sync::OnceLock::new();

static PANIC_HOOK_INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// Install panic hook that writes to both stderr and a fallback file.
/// This is called separately from init_tracing to ensure panics are always logged.
pub fn install_panic_hook() {
    PANIC_HOOK_INSTALLED.get_or_init(|| {
        let default_panic = std::panic::take_hook();

        std::panic::set_hook(Box::new(move |panic_info| {
            let payload = panic_info.payload();
            let message = if let Some(s) = payload.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown panic payload".to_string()
            };

            let location = if let Some(location) = panic_info.location() {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            } else {
                "unknown location".to_string()
            };

            // Always log to tracing (if initialized)
            tracing::error!(
                panic.message = %message,
                panic.location = %location,
                "PANIC occurred in FFF"
            );

            // Always print to stderr
            eprintln!("=== FFF PANIC ===");
            eprintln!("Message: {}", message);
            eprintln!("Location: {}", location);
            eprintln!("=================");

            // Try to write to fallback panic log file
            if let Some(cache_dir) = dirs::cache_dir() {
                let panic_log = cache_dir.join("fff_panic.log");
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                let panic_entry = format!(
                    "\n[{}] PANIC at {}\nMessage: {}\n",
                    timestamp, location, message
                );

                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&panic_log)
                    .and_then(|mut f| {
                        use std::io::Write;
                        f.write_all(panic_entry.as_bytes())
                    });

                eprintln!("Panic logged to: {}", panic_log.display());
            }

            default_panic(panic_info);
        }));
    });
}

/// Parse a log level string into a `tracing::Level`.
///
/// Accepts "trace", "debug", "info", "warn", "error" (case-insensitive).
/// Returns `tracing::Level::INFO` for unrecognised values.
pub fn parse_log_level(level: Option<&str>) -> tracing::Level {
    match level.as_ref().map(|s| s.trim().to_lowercase()).as_deref() {
        Some("trace") => tracing::Level::TRACE,
        Some("debug") => tracing::Level::DEBUG,
        Some("info") => tracing::Level::INFO,
        Some("warn") => tracing::Level::WARN,
        Some("error") => tracing::Level::ERROR,
        _ => tracing::Level::INFO,
    }
}

/// Initialize tracing with a single log file.
///
/// Creates the parent directory if it doesn't exist, truncates the log file,
/// and sets up a non-blocking file appender with structured formatting.
///
/// # Arguments
/// * `log_file_path` - Full path to the log file
/// * `log_level` - Log level (trace, debug, info, warn, error)
///
/// # Returns
/// * `Result<String, io::Error>` - Full path to the log file on success
pub fn init_tracing(log_file_path: &str, log_level: Option<&str>) -> Result<String, io::Error> {
    // Install panic hook first (does nothing if already installed)
    install_panic_hook();

    let log_path = Path::new(log_file_path);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file_appender = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true) // creates a new file on every setup
        .open(log_path)?;

    let level = parse_log_level(log_level);

    TRACING_INITIALIZED.get_or_init(|| {
        let (non_blocking_appender, guard) = non_blocking(file_appender);

        let subscriber = tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .with_writer(non_blocking_appender)
                    .with_target(true)
                    .with_thread_ids(false)
                    .with_thread_names(false)
                    // .with_file(true)
                    // .with_line_number(true)
                    .with_ansi(false)
                    .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE),
            )
            .with(
                EnvFilter::builder()
                    .with_default_directive(level.into())
                    .from_env_lossy(),
            );

        if let Err(e) = tracing::subscriber::set_global_default(subscriber) {
            eprintln!("Failed to set tracing subscriber: {}", e);
        } else {
            tracing::info!(
                "FFF tracing initialized with log file: {}",
                log_path.display()
            );
        }

        guard
    });

    Ok(log_file_path.to_string())
}
