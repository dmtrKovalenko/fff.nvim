use crate::error::Error;
use std::path::Path;
use tracing_appender::non_blocking;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

static TRACING_INITIALIZED: std::sync::OnceLock<tracing_appender::non_blocking::WorkerGuard> =
    std::sync::OnceLock::new();

/// Initialize tracing with single log file
///
/// # Arguments
/// * `log_file_path` - Full path to the log file
/// * `log_level` - Log level (trace, debug, info, warn, error)
///
/// # Returns
/// * `Result<String, Error>` - Full path to the log file on success
pub fn init_tracing(log_file_path: &str, log_level: Option<&str>) -> Result<String, Error> {
    let log_path = Path::new(log_file_path);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file_appender = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true) // creates a new file on every setup
        .open(log_path)?;

    let level = match log_level
        .as_ref()
        .map(|s| s.trim().to_lowercase())
        .as_deref()
    {
        Some("trace") => tracing::Level::TRACE,
        Some("debug") => tracing::Level::DEBUG,
        Some("info") => tracing::Level::INFO,
        Some("warn") => tracing::Level::WARN,
        Some("error") => tracing::Level::ERROR,
        _ => tracing::Level::INFO,
    };

    TRACING_INITIALIZED.get_or_init(|| {
        let (non_blocking_appender, guard) = non_blocking(file_appender);

        let subscriber = tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .with_writer(non_blocking_appender)
                    .with_target(true)
                    .with_thread_ids(false)
                    .with_thread_names(false)
                    .with_file(true)
                    .with_line_number(true)
                    .with_ansi(false),
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
                "FFF.nvim tracing initialized with log file: {}",
                log_path.display()
            );
        }

        std::panic::set_hook(Box::new(|panic_info| {
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

            tracing::error!(
                panic.message = %message,
                panic.location = %location,
                "PANIC occurred in FFF.nvim"
            );

            eprintln!("FFF.nvim PANIC: {} at {}", message, location);
        }));

        guard
    });

    Ok(log_file_path.to_string())
}
