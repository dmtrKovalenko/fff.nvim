use crate::db_healthcheck::DbHealthChecker;
use crate::error::Error;
use crate::file_picker::{FilePicker, FuzzySearchOptions};
use crate::frecency::FrecencyTracker;
use crate::query_tracker::QueryTracker;
use crate::types::PaginationArgs;
use mlua::prelude::*;
use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::Duration;

mod background_watcher;
mod db_healthcheck;
mod error;
pub mod file_picker;
mod frecency;
pub mod git;
mod location;
mod log;
mod path_utils;
pub mod query_tracker;
pub mod score;
mod sort_buffer;
pub mod types;
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub static FRECENCY: Lazy<RwLock<Option<FrecencyTracker>>> = Lazy::new(|| RwLock::new(None));
pub static FILE_PICKER: Lazy<RwLock<Option<FilePicker>>> = Lazy::new(|| RwLock::new(None));
pub static QUERY_TRACKER: Lazy<RwLock<Option<QueryTracker>>> = Lazy::new(|| RwLock::new(None));

pub fn init_db(
    _: &Lua,
    (frecency_db_path, history_db_path, use_unsafe_no_lock): (String, String, bool),
) -> LuaResult<bool> {
    let mut frecency = FRECENCY.write().map_err(|_| Error::AcquireFrecencyLock)?;
    if frecency.is_some() {
        *frecency = None;
    }
    *frecency = Some(FrecencyTracker::new(&frecency_db_path, use_unsafe_no_lock)?);
    tracing::info!("Frecency database initialized at {}", frecency_db_path);

    let mut query_tracker = QUERY_TRACKER
        .write()
        .map_err(|_| Error::AcquireFrecencyLock)?;
    if query_tracker.is_some() {
        *query_tracker = None;
    }

    let tracker = QueryTracker::new(&history_db_path, use_unsafe_no_lock)?;
    *query_tracker = Some(tracker);
    tracing::info!("Query tracker database initialized at {}", history_db_path);

    Ok(true)
}

pub fn destroy_frecency_db(_: &Lua, _: ()) -> LuaResult<bool> {
    let mut frecency = FRECENCY.write().map_err(|_| Error::AcquireFrecencyLock)?;
    *frecency = None;
    Ok(true)
}

pub fn destroy_query_db(_: &Lua, _: ()) -> LuaResult<bool> {
    let mut query_tracker = QUERY_TRACKER
        .write()
        .map_err(|_| Error::AcquireFrecencyLock)?;
    *query_tracker = None;
    Ok(true)
}

pub fn init_file_picker(_: &Lua, base_path: String) -> LuaResult<bool> {
    let mut file_picker = FILE_PICKER.write().map_err(|_| Error::AcquireItemLock)?;
    if file_picker.is_some() {
        return Ok(false);
    }

    let picker = FilePicker::new(base_path)?;
    *file_picker = Some(picker);
    Ok(true)
}

fn reinit_file_picker_internal(path: &Path) -> Result<(), Error> {
    let mut file_picker = FILE_PICKER.write().map_err(|_| Error::AcquireItemLock)?;

    // drop should clean it anyway but just to be extra sure
    if let Some(mut picker) = file_picker.take() {
        picker.stop_background_monitor();
    }

    let new_picker = FilePicker::new(path.to_string_lossy().to_string())?;
    *file_picker = Some(new_picker);

    Ok(())
}

pub fn restart_index_in_path(_: &Lua, new_path: String) -> LuaResult<()> {
    let path = std::path::PathBuf::from(&new_path);
    if !path.exists() {
        return Err(LuaError::RuntimeError(format!(
            "Path does not exist: {}",
            new_path
        )));
    }

    let canonical_path = path.canonicalize().map_err(|e| {
        LuaError::RuntimeError(format!("Failed to canonicalize path '{}': {}", new_path, e))
    })?;

    // Spawn a background thread to avoid blocking Lua/UI thread
    std::thread::spawn(move || {
        if let Err(e) = reinit_file_picker_internal(&canonical_path) {
            ::tracing::error!(
                ?e,
                ?canonical_path,
                "Failed to index directory after changing"
            );
        } else {
            ::tracing::info!(?canonical_path, "Successfully reindexed directory");
        }
    });

    Ok(())
}

pub fn scan_files(_: &Lua, _: ()) -> LuaResult<()> {
    let mut file_picker = FILE_PICKER.write().map_err(|_| Error::AcquireItemLock)?;
    let picker = file_picker
        .as_mut()
        .ok_or_else(|| Error::FilePickerMissing)?;

    picker.trigger_rescan()?;
    ::tracing::info!("scan_files trigger_rescan completed");
    Ok(())
}

#[allow(clippy::type_complexity)]
pub fn fuzzy_search_files(
    lua: &Lua,
    (
        query,
        max_threads,
        current_file,
        combo_boost_score_multiplier,
        min_combo_count,
        page_index,
        page_size,
    ): (
        String,
        usize,
        Option<String>,
        i32,
        Option<u32>,
        Option<usize>,
        Option<usize>,
    ),
) -> LuaResult<LuaValue> {
    let Some(ref mut picker) = *FILE_PICKER.write().map_err(|_| Error::AcquireItemLock)? else {
        return Err(Error::FilePickerMissing)?;
    };

    let base_path = picker.base_path();
    let min_combo_count = min_combo_count.unwrap_or(3);

    let last_same_query_entry = {
        let query_tracker = QUERY_TRACKER
            .read()
            .map_err(|_| Error::AcquireFrecencyLock)?;

        if query_tracker.as_ref().is_none() {
            tracing::warn!("Query tracker not initialized");
        }

        query_tracker
            .as_ref()
            .map(|tracker| tracker.get_last_query_entry(&query, base_path, min_combo_count))
            .transpose()?
            .flatten()
    };

    tracing::debug!(
        ?last_same_query_entry,
        ?base_path,
        ?query,
        ?min_combo_count,
        ?page_index,
        ?page_size,
        "Fuzzy search parameters"
    );

    let results = FilePicker::fuzzy_search(
        picker.get_files(),
        &query,
        FuzzySearchOptions {
            max_threads,
            current_file: current_file.as_deref(),
            project_path: Some(picker.base_path()),
            last_same_query_match: last_same_query_entry.as_ref(),
            combo_boost_score_multiplier,
            min_combo_count,
            pagination: PaginationArgs {
                offset: page_index.unwrap_or(0),
                limit: page_size.unwrap_or(0),
            },
        },
    );

    results.into_lua(lua)
}

pub fn track_access(_: &Lua, file_path: String) -> LuaResult<bool> {
    let file_path = PathBuf::from(&file_path);

    // Track access in frecency DB (expensive LMDB write, ~100-200ms)
    // Do this WITHOUT holding FILE_PICKER lock to avoid blocking searches
    let Some(ref frecency) = *FRECENCY.read().map_err(|_| Error::AcquireFrecencyLock)? else {
        return Ok(false);
    };
    frecency.track_access(file_path.as_path())?;

    // Quick lock to update single file's frecency score in picker
    let Some(ref mut picker) = *FILE_PICKER.write().map_err(|_| Error::AcquireItemLock)? else {
        return Err(Error::FilePickerMissing)?;
    };
    picker.update_single_file_frecency(&file_path, frecency)?;

    Ok(true)
}

pub fn get_scan_progress(lua: &Lua, _: ()) -> LuaResult<LuaValue> {
    let file_picker = FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)?;
    let picker = file_picker
        .as_ref()
        .ok_or_else(|| Error::FilePickerMissing)?;
    let progress = picker.get_scan_progress();

    let table = lua.create_table()?;
    table.set("scanned_files_count", progress.scanned_files_count)?;
    table.set("is_scanning", progress.is_scanning)?;
    Ok(LuaValue::Table(table))
}

pub fn is_scanning(_: &Lua, _: ()) -> LuaResult<bool> {
    let file_picker = FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)?;
    let picker = file_picker
        .as_ref()
        .ok_or_else(|| Error::FilePickerMissing)?;
    Ok(picker.is_scan_active())
}

pub fn refresh_git_status(_: &Lua, _: ()) -> LuaResult<usize> {
    FilePicker::refresh_git_status_global().map_err(Into::into)
}

pub fn update_single_file_frecency(_: &Lua, file_path: String) -> LuaResult<bool> {
    let Some(ref frecency) = *FRECENCY.read().map_err(|_| Error::AcquireFrecencyLock)? else {
        return Ok(false);
    };
    let Some(ref mut picker) = *FILE_PICKER.write().map_err(|_| Error::AcquireItemLock)? else {
        return Err(Error::FilePickerMissing)?;
    };

    picker.update_single_file_frecency(&file_path, frecency)?;
    Ok(true)
}

pub fn stop_background_monitor(_: &Lua, _: ()) -> LuaResult<bool> {
    let Some(ref mut picker) = *FILE_PICKER.write().map_err(|_| Error::AcquireItemLock)? else {
        return Err(Error::FilePickerMissing)?;
    };

    picker.stop_background_monitor();

    Ok(true)
}

pub fn cleanup_file_picker(_: &Lua, _: ()) -> LuaResult<bool> {
    let mut file_picker = FILE_PICKER.write().map_err(|_| Error::AcquireItemLock)?;
    if let Some(picker) = file_picker.take() {
        drop(picker);
        ::tracing::info!("FilePicker cleanup completed");

        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn cancel_scan(_: &Lua, _: ()) -> LuaResult<bool> {
    Ok(true)
}

pub fn track_query_completion(_: &Lua, (query, file_path): (String, String)) -> LuaResult<bool> {
    // Get the project path before spawning thread
    let project_path = {
        let Some(ref picker) = *FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)? else {
            return Ok(false);
        };
        picker.base_path().to_path_buf()
    };

    // Canonicalize the file path before spawning thread
    let file_path = match PathBuf::from(&file_path).canonicalize() {
        Ok(path) => path,
        Err(e) => {
            tracing::warn!(?file_path, error = ?e, "Failed to canonicalize file path for tracking");
            return Ok(false);
        }
    };

    // Spawn background thread to do the actual tracking (expensive DB write)
    std::thread::spawn(move || {
        if let Ok(Some(tracker)) = QUERY_TRACKER.write().as_deref_mut()
            && let Err(e) = tracker.track_query_completion(&query, &project_path, &file_path)
        {
            tracing::error!(
                query = %query,
                file = %file_path.display(),
                error = ?e,
                "Failed to track query completion"
            );
        }
    });

    Ok(true)
}

pub fn get_historical_query(_: &Lua, offset: usize) -> LuaResult<Option<String>> {
    let project_path = {
        let Some(ref picker) = *FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)? else {
            return Ok(None);
        };
        picker.base_path().to_path_buf()
    };

    let Some(ref tracker) = *QUERY_TRACKER
        .read()
        .map_err(|_| Error::AcquireFrecencyLock)?
    else {
        return Ok(None);
    };

    tracker
        .get_historical_query(&project_path, offset)
        .map_err(Into::into)
}

pub fn wait_for_initial_scan(_: &Lua, timeout_ms: Option<u64>) -> LuaResult<bool> {
    let file_picker = FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)?;
    let picker = file_picker
        .as_ref()
        .ok_or_else(|| Error::FilePickerMissing)?;

    let timeout_ms = timeout_ms.unwrap_or(500);
    let timeout_duration = Duration::from_millis(timeout_ms);
    let start_time = std::time::Instant::now();
    let mut sleep_duration = Duration::from_millis(1);

    while picker.is_scan_active() {
        if start_time.elapsed() >= timeout_duration {
            ::tracing::warn!("wait_for_initial_scan timed out after {}ms", timeout_ms);
            return Ok(false);
        }

        std::thread::sleep(sleep_duration);
        sleep_duration = std::cmp::min(sleep_duration * 2, Duration::from_millis(50));
    }

    ::tracing::debug!(
        "wait_for_initial_scan completed in {:?}",
        start_time.elapsed()
    );
    Ok(true)
}

pub fn init_tracing(
    _: &Lua,
    (log_file_path, log_level): (String, Option<String>),
) -> LuaResult<String> {
    crate::log::init_tracing(&log_file_path, log_level.as_deref())
        .map_err(|e| LuaError::RuntimeError(format!("Failed to initialize tracing: {}", e)))
}

/// Returns health check information including version, git2 status, and repository detection
pub fn health_check(lua: &Lua, test_path: Option<String>) -> LuaResult<LuaValue> {
    let table = lua.create_table()?;
    table.set("version", env!("CARGO_PKG_VERSION"))?;

    let test_path = test_path
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let git_info = lua.create_table()?;
    let git_version = git2::Version::get();
    let (major, minor, rev) = git_version.libgit2_version();
    let libgit2_version_str = format!("{}.{}.{}", major, minor, rev);

    match git2::Repository::discover(&test_path) {
        Ok(repo) => {
            git_info.set("available", true)?;
            git_info.set("repository_found", true)?;
            if let Some(workdir) = repo.workdir() {
                git_info.set("workdir", workdir.to_string_lossy().to_string())?;
            }
            // Get git2 version info
            git_info.set("libgit2_version", libgit2_version_str.clone())?;
        }
        Err(e) => {
            git_info.set("available", true)?;
            git_info.set("repository_found", false)?;
            git_info.set("error", e.message().to_string())?;
            git_info.set("libgit2_version", libgit2_version_str)?;
        }
    }
    table.set("git", git_info)?;

    // Check file picker status
    let picker_info = lua.create_table()?;
    match FILE_PICKER.read() {
        Ok(guard) => {
            if let Some(ref picker) = *guard {
                picker_info.set("initialized", true)?;
                picker_info.set(
                    "base_path",
                    picker.base_path().to_string_lossy().to_string(),
                )?;
                picker_info.set("is_scanning", picker.is_scan_active())?;
                let progress = picker.get_scan_progress();
                picker_info.set("indexed_files", progress.scanned_files_count)?;
            } else {
                picker_info.set("initialized", false)?;
            }
        }
        Err(_) => {
            picker_info.set("initialized", false)?;
            picker_info.set("error", "Failed to acquire file picker lock")?;
        }
    }
    table.set("file_picker", picker_info)?;

    let frecency_info = lua.create_table()?;
    match FRECENCY.read() {
        Ok(guard) => {
            frecency_info.set("initialized", guard.is_some())?;

            if let Some(ref frecency) = *guard {
                match frecency.get_lua_helthcheckh(lua) {
                    Ok(healthcheck_table) => {
                        frecency_info.set("db_healthcheck", healthcheck_table)?;
                    }
                    Err(e) => {
                        frecency_info.set("db_healthcheck_error", e.to_string())?;
                    }
                }
            }
        }
        Err(_) => {
            frecency_info.set("initialized", false)?;
            frecency_info.set("error", "Failed to acquire frecency lock")?;
        }
    }
    table.set("frecency", frecency_info)?;

    let query_tracker_info = lua.create_table()?;
    match QUERY_TRACKER.read() {
        Ok(guard) => {
            query_tracker_info.set("initialized", guard.is_some())?;
            if let Some(ref query_history) = *guard {
                match query_history.get_lua_helthcheckh(lua) {
                    Ok(healthcheck_table) => {
                        query_tracker_info.set("db_healthcheck", healthcheck_table)?;
                    }
                    Err(e) => {
                        query_tracker_info.set("db_healthcheck_error", e.to_string())?;
                    }
                }
            }
        }
        Err(_) => {
            query_tracker_info.set("initialized", false)?;
            query_tracker_info.set("error", "Failed to acquire query tracker lock")?;
        }
    }
    table.set("query_tracker", query_tracker_info)?;

    Ok(LuaValue::Table(table))
}

fn create_exports(lua: &Lua) -> LuaResult<LuaTable> {
    let exports = lua.create_table()?;
    exports.set("init_db", lua.create_function(init_db)?)?;
    exports.set(
        "destroy_frecency_db",
        lua.create_function(destroy_frecency_db)?,
    )?;
    exports.set("init_file_picker", lua.create_function(init_file_picker)?)?;
    exports.set(
        "restart_index_in_path",
        lua.create_function(restart_index_in_path)?,
    )?;
    exports.set("scan_files", lua.create_function(scan_files)?)?;
    exports.set(
        "fuzzy_search_files",
        lua.create_function(fuzzy_search_files)?,
    )?;
    exports.set("track_access", lua.create_function(track_access)?)?;
    exports.set("cancel_scan", lua.create_function(cancel_scan)?)?;
    exports.set("get_scan_progress", lua.create_function(get_scan_progress)?)?;
    exports.set(
        "refresh_git_status",
        lua.create_function(refresh_git_status)?,
    )?;
    exports.set(
        "stop_background_monitor",
        lua.create_function(stop_background_monitor)?,
    )?;
    exports.set("init_tracing", lua.create_function(init_tracing)?)?;
    exports.set(
        "wait_for_initial_scan",
        lua.create_function(wait_for_initial_scan)?,
    )?;
    exports.set(
        "cleanup_file_picker",
        lua.create_function(cleanup_file_picker)?,
    )?;
    exports.set("destroy_query_db", lua.create_function(destroy_query_db)?)?;
    exports.set(
        "track_query_completion",
        lua.create_function(track_query_completion)?,
    )?;
    exports.set(
        "get_historical_query",
        lua.create_function(get_historical_query)?,
    )?;
    exports.set("health_check", lua.create_function(health_check)?)?;

    Ok(exports)
}

// https://github.com/mlua-rs/mlua/issues/318
#[mlua::lua_module(skip_memory_check)]
fn fff_nvim(lua: &Lua) -> LuaResult<LuaTable> {
    // Install panic hook IMMEDIATELY on module load
    // This ensures any panics are logged even if init_tracing is never called
    crate::log::install_panic_hook();

    create_exports(lua)
}
