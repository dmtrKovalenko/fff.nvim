use crate::path_shortening::shorten_path_with_cache;
use error::{IntoCoreError, IntoLuaResult};
use fff_core::file_picker::FilePicker;
use fff_core::frecency::FrecencyTracker;
use fff_core::query_tracker::QueryTracker;
use fff_core::{
    DbHealthChecker, Error, FuzzySearchOptions, PaginationArgs, QueryParser, SharedFrecency,
    SharedPicker, SharedQueryTracker,
};
use mimalloc::MiMalloc;
use mlua::prelude::*;
use once_cell::sync::Lazy;
use path_shortening::PathShortenStrategy;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

mod error;
mod log;
mod lua_types;
mod path_shortening;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

// the global state for neovim lives here for efficiency
// lua ffi is pretty bad with the overhead of converting raw pointer into tables
pub static FILE_PICKER: Lazy<SharedPicker> = Lazy::new(|| Arc::new(RwLock::new(None)));
pub static FRECENCY: Lazy<SharedFrecency> = Lazy::new(|| Arc::new(RwLock::new(None)));
pub static QUERY_TRACKER: Lazy<SharedQueryTracker> = Lazy::new(|| Arc::new(RwLock::new(None)));

pub fn init_db(
    _: &Lua,
    (frecency_db_path, history_db_path, use_unsafe_no_lock): (String, String, bool),
) -> LuaResult<bool> {
    let mut frecency = FRECENCY
        .write()
        .with_lock_error(Error::AcquireFrecencyLock)
        .into_lua_result()?;
    if frecency.is_some() {
        *frecency = None;
    }
    *frecency =
        Some(FrecencyTracker::new(&frecency_db_path, use_unsafe_no_lock).into_lua_result()?);
    tracing::info!("Frecency database initialized at {}", frecency_db_path);

    let mut query_tracker = QUERY_TRACKER
        .write()
        .with_lock_error(Error::AcquireFrecencyLock)
        .into_lua_result()?;
    if query_tracker.is_some() {
        *query_tracker = None;
    }

    let tracker = QueryTracker::new(&history_db_path, use_unsafe_no_lock).into_lua_result()?;
    *query_tracker = Some(tracker);
    tracing::info!("Query tracker database initialized at {}", history_db_path);

    Ok(true)
}

pub fn destroy_frecency_db(_: &Lua, _: ()) -> LuaResult<bool> {
    let mut frecency = FRECENCY
        .write()
        .with_lock_error(Error::AcquireFrecencyLock)
        .into_lua_result()?;
    *frecency = None;
    Ok(true)
}

pub fn destroy_query_db(_: &Lua, _: ()) -> LuaResult<bool> {
    let mut query_tracker = QUERY_TRACKER
        .write()
        .with_lock_error(Error::AcquireFrecencyLock)
        .into_lua_result()?;
    *query_tracker = None;
    Ok(true)
}

pub fn init_file_picker(_: &Lua, base_path: String) -> LuaResult<bool> {
    {
        let guard = FILE_PICKER
            .read()
            .with_lock_error(Error::AcquireItemLock)
            .into_lua_result()?;
        if guard.is_some() {
            return Ok(false);
        }
    }

    FilePicker::new_with_shared_state(
        base_path,
        false,
        Arc::clone(&FILE_PICKER),
        Arc::clone(&FRECENCY),
    )
    .into_lua_result()?;

    Ok(true)
}

fn reinit_file_picker_internal(path: &Path) -> Result<(), Error> {
    // Stop existing picker
    {
        let mut guard = FILE_PICKER
            .write()
            .with_lock_error(Error::AcquireItemLock)?;
        if let Some(mut picker) = guard.take() {
            picker.stop_background_monitor();
        }
    }

    // Create new picker backed by the same shared state
    FilePicker::new_with_shared_state(
        path.to_string_lossy().to_string(),
        false,
        Arc::clone(&FILE_PICKER),
        Arc::clone(&FRECENCY),
    )?;

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

    let canonical_path = fff_core::path_utils::canonicalize(&path).map_err(|e| {
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
    let mut file_picker = FILE_PICKER
        .write()
        .with_lock_error(Error::AcquireItemLock)
        .into_lua_result()?;
    let picker = file_picker
        .as_mut()
        .ok_or(Error::FilePickerMissing)
        .into_lua_result()?;

    picker.trigger_rescan(&FRECENCY).into_lua_result()?;
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
    let file_picker_guard = FILE_PICKER
        .read()
        .with_lock_error(Error::AcquireItemLock)
        .into_lua_result()?;
    let Some(ref picker) = *file_picker_guard else {
        return Err(error::to_lua_error(Error::FilePickerMissing));
    };

    let base_path = picker.base_path();
    let min_combo_count = min_combo_count.unwrap_or(3);

    let last_same_query_entry = {
        let query_tracker = QUERY_TRACKER
            .read()
            .with_lock_error(Error::AcquireFrecencyLock)
            .into_lua_result()?;

        if query_tracker.as_ref().is_none() {
            tracing::warn!("Query tracker not initialized");
        }

        query_tracker
            .as_ref()
            .map(|tracker| tracker.get_last_query_entry(&query, base_path, min_combo_count))
            .transpose()
            .into_lua_result()?
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

    // Parse the query once at the API boundary
    let parser = QueryParser::default();
    let parsed = parser.parse(&query);

    let results = FilePicker::fuzzy_search(
        picker.get_files(),
        &query,
        parsed,
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

    lua_types::SearchResultLua::from(results).into_lua(lua)
}

#[allow(clippy::type_complexity)]
pub fn live_grep(
    lua: &Lua,
    (
        query,
        file_offset,
        page_size,
        max_file_size,
        max_matches_per_file,
        smart_case,
        grep_mode,
        time_budget_ms,
    ): (
        String,
        Option<usize>,
        Option<usize>,
        Option<u64>,
        Option<usize>,
        Option<bool>,
        Option<String>,
        Option<u64>,
    ),
) -> LuaResult<LuaValue> {
    let file_picker_guard = FILE_PICKER
        .read()
        .with_lock_error(Error::AcquireItemLock)
        .into_lua_result()?;
    let Some(ref picker) = *file_picker_guard else {
        return Err(error::to_lua_error(Error::FilePickerMissing));
    };

    let parsed = fff_core::grep::parse_grep_query(&query);

    let mode = match grep_mode.as_deref() {
        Some("regex") => fff_core::GrepMode::Regex,
        Some("fuzzy") => fff_core::GrepMode::Fuzzy,
        _ => fff_core::GrepMode::PlainText, // "plain" or nil or unknown
    };

    let options = fff_core::GrepSearchOptions {
        max_file_size: max_file_size.unwrap_or(10 * 1024 * 1024),
        max_matches_per_file: max_matches_per_file.unwrap_or(200),
        smart_case: smart_case.unwrap_or(true),
        file_offset: file_offset.unwrap_or(0),
        page_limit: page_size.unwrap_or(50),
        mode,
        time_budget_ms: time_budget_ms.unwrap_or(0),
    };

    let result = fff_core::grep::grep_search(picker.get_files(), &query, parsed, &options);

    lua_types::GrepResultLua::from(result).into_lua(lua)
}

pub fn track_access(_: &Lua, file_path: String) -> LuaResult<bool> {
    let file_path = PathBuf::from(&file_path);

    // Track access in frecency DB (expensive LMDB write, ~100-200ms)
    // Do this WITHOUT holding FILE_PICKER lock to avoid blocking searches
    let frecency_guard = FRECENCY
        .read()
        .with_lock_error(Error::AcquireFrecencyLock)
        .into_lua_result()?;
    let Some(ref frecency) = *frecency_guard else {
        return Ok(false);
    };
    frecency
        .track_access(file_path.as_path())
        .into_lua_result()?;
    drop(frecency_guard);

    // Quick lock to update single file's frecency score in picker
    let mut file_picker = FILE_PICKER
        .write()
        .with_lock_error(Error::AcquireItemLock)
        .into_lua_result()?;
    let Some(ref mut picker) = *file_picker else {
        return Err(error::to_lua_error(Error::FilePickerMissing));
    };

    let frecency_guard = FRECENCY
        .read()
        .with_lock_error(Error::AcquireFrecencyLock)
        .into_lua_result()?;
    let Some(ref frecency) = *frecency_guard else {
        return Ok(false);
    };
    picker
        .update_single_file_frecency(&file_path, frecency)
        .into_lua_result()?;

    Ok(true)
}

pub fn get_scan_progress(lua: &Lua, _: ()) -> LuaResult<LuaValue> {
    let file_picker = FILE_PICKER
        .read()
        .with_lock_error(Error::AcquireItemLock)
        .into_lua_result()?;
    let picker = file_picker
        .as_ref()
        .ok_or(Error::FilePickerMissing)
        .into_lua_result()?;
    let progress = picker.get_scan_progress();

    let table = lua.create_table()?;
    table.set("scanned_files_count", progress.scanned_files_count)?;
    table.set("is_scanning", progress.is_scanning)?;
    Ok(LuaValue::Table(table))
}

pub fn is_scanning(_: &Lua, _: ()) -> LuaResult<bool> {
    let file_picker = FILE_PICKER
        .read()
        .with_lock_error(Error::AcquireItemLock)
        .into_lua_result()?;
    let picker = file_picker
        .as_ref()
        .ok_or(Error::FilePickerMissing)
        .into_lua_result()?;
    Ok(picker.is_scan_active())
}

pub fn get_git_root(_: &Lua, _: ()) -> LuaResult<Option<String>> {
    let file_picker = FILE_PICKER
        .read()
        .with_lock_error(Error::AcquireItemLock)
        .into_lua_result()?;
    let Some(ref picker) = *file_picker else {
        return Ok(None);
    };

    Ok(picker.git_root().map(|p| p.to_string_lossy().into_owned()))
}

pub fn refresh_git_status(_: &Lua, _: ()) -> LuaResult<usize> {
    FilePicker::refresh_git_status(&FILE_PICKER, &FRECENCY).into_lua_result()
}

pub fn update_single_file_frecency(_: &Lua, file_path: String) -> LuaResult<bool> {
    let frecency_guard = FRECENCY
        .read()
        .with_lock_error(Error::AcquireFrecencyLock)
        .into_lua_result()?;
    let Some(ref frecency) = *frecency_guard else {
        return Ok(false);
    };

    let mut file_picker = FILE_PICKER
        .write()
        .with_lock_error(Error::AcquireItemLock)
        .into_lua_result()?;
    let Some(ref mut picker) = *file_picker else {
        return Err(error::to_lua_error(Error::FilePickerMissing));
    };

    picker
        .update_single_file_frecency(&file_path, frecency)
        .into_lua_result()?;
    Ok(true)
}

pub fn stop_background_monitor(_: &Lua, _: ()) -> LuaResult<bool> {
    let mut file_picker = FILE_PICKER
        .write()
        .with_lock_error(Error::AcquireItemLock)
        .into_lua_result()?;
    let Some(ref mut picker) = *file_picker else {
        return Err(error::to_lua_error(Error::FilePickerMissing));
    };

    picker.stop_background_monitor();

    Ok(true)
}

pub fn cleanup_file_picker(_: &Lua, _: ()) -> LuaResult<bool> {
    let mut file_picker = FILE_PICKER
        .write()
        .with_lock_error(Error::AcquireItemLock)
        .into_lua_result()?;
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
        let file_picker = FILE_PICKER
            .read()
            .with_lock_error(Error::AcquireItemLock)
            .into_lua_result()?;
        let Some(ref picker) = *file_picker else {
            return Ok(false);
        };
        picker.base_path().to_path_buf()
    };

    // Canonicalize the file path before spawning thread
    let file_path = match fff_core::path_utils::canonicalize(&file_path) {
        Ok(path) => path,
        Err(e) => {
            tracing::warn!(?file_path, error = ?e, "Failed to canonicalize file path for tracking");
            return Ok(false);
        }
    };

    // Spawn background thread to do the actual tracking (expensive DB write)
    let query_tracker = Arc::clone(&QUERY_TRACKER);
    std::thread::spawn(move || {
        if let Ok(Some(tracker)) = query_tracker.write().as_deref_mut()
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
        let file_picker = FILE_PICKER
            .read()
            .with_lock_error(Error::AcquireItemLock)
            .into_lua_result()?;
        let Some(ref picker) = *file_picker else {
            return Ok(None);
        };
        picker.base_path().to_path_buf()
    };

    let query_tracker = QUERY_TRACKER
        .read()
        .with_lock_error(Error::AcquireFrecencyLock)
        .into_lua_result()?;
    let Some(ref tracker) = *query_tracker else {
        return Ok(None);
    };

    tracker
        .get_historical_query(&project_path, offset)
        .into_lua_result()
}

pub fn track_grep_query(_: &Lua, query: String) -> LuaResult<bool> {
    let project_path = {
        let file_picker = FILE_PICKER
            .read()
            .with_lock_error(Error::AcquireItemLock)
            .into_lua_result()?;
        let Some(ref picker) = *file_picker else {
            return Ok(false);
        };
        picker.base_path().to_path_buf()
    };

    let query_tracker = Arc::clone(&QUERY_TRACKER);
    std::thread::spawn(move || {
        if let Ok(Some(tracker)) = query_tracker.write().as_deref_mut()
            && let Err(e) = tracker.track_grep_query(&query, &project_path)
        {
            tracing::error!(
                query = %query,
                error = ?e,
                "Failed to track grep query"
            );
        }
    });

    Ok(true)
}

pub fn get_historical_grep_query(_: &Lua, offset: usize) -> LuaResult<Option<String>> {
    let project_path = {
        let file_picker = FILE_PICKER
            .read()
            .with_lock_error(Error::AcquireItemLock)
            .into_lua_result()?;
        let Some(ref picker) = *file_picker else {
            return Ok(None);
        };
        picker.base_path().to_path_buf()
    };

    let query_tracker = QUERY_TRACKER
        .read()
        .with_lock_error(Error::AcquireFrecencyLock)
        .into_lua_result()?;
    let Some(ref tracker) = *query_tracker else {
        return Ok(None);
    };

    tracker
        .get_historical_grep_query(&project_path, offset)
        .into_lua_result()
}

pub fn wait_for_initial_scan(_: &Lua, timeout_ms: Option<u64>) -> LuaResult<bool> {
    let file_picker = FILE_PICKER
        .read()
        .with_lock_error(Error::AcquireItemLock)
        .into_lua_result()?;
    let picker = file_picker
        .as_ref()
        .ok_or(Error::FilePickerMissing)
        .into_lua_result()?;

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
                match frecency.get_health() {
                    Ok(health) => {
                        let healthcheck_table = lua.create_table()?;
                        healthcheck_table.set("path", health.path)?;
                        healthcheck_table.set("disk_size", health.disk_size)?;
                        for (name, count) in health.entry_counts {
                            healthcheck_table.set(name, count)?;
                        }
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
                match query_history.get_health() {
                    Ok(health) => {
                        let healthcheck_table = lua.create_table()?;
                        healthcheck_table.set("path", health.path)?;
                        healthcheck_table.set("disk_size", health.disk_size)?;
                        for (name, count) in health.entry_counts {
                            healthcheck_table.set(name, count)?;
                        }
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

pub fn shorten_path(
    _: &Lua,
    (path, max_size, strategy): (String, usize, Option<mlua::Value>),
) -> LuaResult<String> {
    let strategy = strategy
        .map(|v| -> LuaResult<PathShortenStrategy> {
            match v {
                mlua::Value::String(ref s) => {
                    let name = s
                        .to_str()
                        .map(|s| s.to_owned())
                        .unwrap_or_else(|_| "middle_number".to_string());
                    Ok(PathShortenStrategy::from_name(&name))
                }
                _ => Ok(PathShortenStrategy::default()),
            }
        })
        .transpose()?
        .unwrap_or_default();

    shorten_path_with_cache(strategy, max_size, Path::new(&path)).map_err(LuaError::RuntimeError)
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
    exports.set("live_grep", lua.create_function(live_grep)?)?;
    exports.set("track_access", lua.create_function(track_access)?)?;
    exports.set("cancel_scan", lua.create_function(cancel_scan)?)?;
    exports.set("get_scan_progress", lua.create_function(get_scan_progress)?)?;
    exports.set(
        "refresh_git_status",
        lua.create_function(refresh_git_status)?,
    )?;
    exports.set("get_git_root", lua.create_function(get_git_root)?)?;
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
    exports.set("track_grep_query", lua.create_function(track_grep_query)?)?;
    exports.set(
        "get_historical_grep_query",
        lua.create_function(get_historical_grep_query)?,
    )?;
    exports.set("health_check", lua.create_function(health_check)?)?;
    exports.set("shorten_path", lua.create_function(shorten_path)?)?;

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
