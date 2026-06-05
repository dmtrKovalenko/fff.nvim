use std::path::PathBuf;

use fff::{FFFMode, FilePickerOptions, SharedFilePicker, SharedFrecency};
use fff::file_picker::FilePicker;
use fff::frecency::FrecencyTracker;
use git2::Repository;

/// Resolved arguments after merging CLI flags with config file values.
/// Passed to init() so state.rs doesn't need to know about config loading.
pub struct EffectiveArgs {
    pub base_path: PathBuf,
    pub frecency_db_path: Option<PathBuf>,
    pub no_watch: bool,
    pub no_warmup: bool,
}

pub struct EngineState {
    pub shared_picker: SharedFilePicker,
    /// Retained for KTD-5 RecordAccess follow-on track.
    #[allow(dead_code)]
    pub shared_frecency: SharedFrecency,
    /// Retained for use by lifecycle / health-check paths.
    #[allow(dead_code)]
    pub base_path: PathBuf,
}

pub fn init(args: &EffectiveArgs) -> Result<EngineState, Box<dyn std::error::Error>> {
    let base_path = resolve_base_path(&args.base_path);

    let shared_picker = SharedFilePicker::default();
    let shared_frecency = SharedFrecency::default();

    // R5: frecency enabled by default — always open LMDB.
    let frecency_path = args
        .frecency_db_path
        .clone()
        .unwrap_or_else(default_frecency_path);
    std::fs::create_dir_all(&frecency_path)?;

    match FrecencyTracker::open(&frecency_path) {
        Ok(tracker) => {
            let _ = shared_frecency.init(tracker);
            tracing::info!("Frecency DB opened at {}", frecency_path.display());
        }
        Err(e) => {
            tracing::warn!("Failed to open frecency DB at {}: {e}", frecency_path.display());
        }
    }

    let enable_content_indexing = !args.no_warmup;

    FilePicker::new_with_shared_state(
        shared_picker.clone(),
        shared_frecency.clone(),
        FilePickerOptions {
            base_path: base_path.to_string_lossy().to_string(),
            enable_mmap_cache: !args.no_warmup,
            enable_content_indexing,
            watch: !args.no_watch,
            mode: FFFMode::Ai,
            follow_symlinks: false,
            ..Default::default()
        },
    )?;

    tracing::info!("FilePicker initialized for {}", base_path.display());

    Ok(EngineState {
        shared_picker,
        shared_frecency,
        base_path,
    })
}

fn resolve_base_path(supplied: &std::path::Path) -> PathBuf {
    let s = supplied.to_string_lossy();
    match Repository::discover(&*s) {
        Ok(repo) => {
            if let Some(workdir) = repo.workdir() {
                tracing::info!("Discovered git root: {}", workdir.display());
                workdir.to_path_buf()
            } else {
                tracing::info!("Git repo is bare, using supplied path");
                supplied.to_path_buf()
            }
        }
        Err(_) => {
            tracing::info!("No git repo found, using supplied path");
            supplied.to_path_buf()
        }
    }
}

fn default_frecency_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".local/share")
        })
        .join("fff")
        .join("frecency")
}
