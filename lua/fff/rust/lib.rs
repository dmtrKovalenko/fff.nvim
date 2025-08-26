use crate::error::Error;
use crate::file_picker::FilePicker;
use crate::frecency::FrecencyTracker;
use crate::search_results::{SearchResult, SearchResultsState};
use crate::types::FileItem;
use mlua::prelude::*;
use once_cell::sync::Lazy;
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::Duration;

mod background_watcher;
mod error;
pub mod file_picker;
mod frecency;
pub mod git;
mod path_utils;
pub mod score;
mod search_results;
mod tracing;
pub mod types;
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub static LAST_SEARCH_RESULTS: Lazy<RwLock<Option<SearchResultsState>>> =
    Lazy::new(|| RwLock::new(None));
pub static FRECENCY: Lazy<RwLock<Option<FrecencyTracker>>> = Lazy::new(|| RwLock::new(None));
pub static FILE_PICKER: Lazy<RwLock<Option<FilePicker>>> = Lazy::new(|| RwLock::new(None));

pub fn init_db(_: &Lua, (db_path, use_unsafe_no_lock): (String, bool)) -> LuaResult<bool> {
    let mut frecency = FRECENCY.write().map_err(|_| Error::AcquireFrecencyLock)?;
    if frecency.is_some() {
        return Ok(false);
    }
    *frecency = Some(FrecencyTracker::new(&db_path, use_unsafe_no_lock)?);
    Ok(true)
}

pub fn destroy_db(_: &Lua, _: ()) -> LuaResult<bool> {
    let mut frecency = FRECENCY.write().map_err(|_| Error::AcquireFrecencyLock)?;
    *frecency = None;
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

fn reinit_file_picker_internal(path: std::path::PathBuf) -> Result<(), Error> {
    let mut file_picker = FILE_PICKER.write().map_err(|_| Error::AcquireItemLock)?;

    // drop should clean it anyway but just to be extra sure
    if let Some(mut picker) = file_picker.take() {
        picker.stop_background_monitor();
    }

    let new_picker = FilePicker::new(path.to_string_lossy().to_string())?;
    *file_picker = Some(new_picker);

    Ok(())
}

pub fn restart_index_in_path(_: &Lua, new_path: String) -> LuaResult<bool> {
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

    reinit_file_picker_internal(canonical_path)?;
    Ok(true)
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

pub fn fuzzy_search_files(
    lua: &Lua,
    (query, max_results, max_threads, current_file, order_reverse): (
        String,
        usize,
        usize,
        Option<String>,
        bool,
    ),
) -> LuaResult<LuaValue> {
    let Some(ref mut picker) = *FILE_PICKER.write().map_err(|_| Error::AcquireItemLock)? else {
        return Err(Error::FilePickerMissing)?;
    };

    let all_files = picker.get_files();
    // let all_files = {
    //
    //     let Some(ref last_search_results) = *LAST_SEARCH_RESULTS
    //         .read()
    //         .map_err(|_| Error::AcquireItemLock)?
    //     else {
    //     };
    //
    //     last_search_results.all_files_to_sort(&query, all_files)
    // }?;

    let results = FilePicker::fuzzy_search(
        &all_files,
        &query,
        max_results,
        max_threads,
        current_file.as_deref(),
        order_reverse,
    );

    let Some(ref mut last_search_results) = *LAST_SEARCH_RESULTS
        .write()
        .map_err(|_| Error::AcquireItemLock)?
    else {
        return Err(Error::FilePickerMissing)?;
    };

    let results = SearchResult::capture_and_truncate_search_results(
        query,
        results,
        last_search_results,
        all_files.len(),
        max_results,
    );

    results.into_lua(lua)
}

pub fn track_access(_: &Lua, file_path: String) -> LuaResult<bool> {
    let Some(ref frecency) = *FRECENCY.read().map_err(|_| Error::AcquireFrecencyLock)? else {
        return Ok(false);
    };
    let Some(ref mut picker) = *FILE_PICKER.write().map_err(|_| Error::AcquireItemLock)? else {
        return Err(Error::FilePickerMissing)?;
    };

    let file_path = PathBuf::from(&file_path).canonicalize()?;
    frecency.track_access(file_path.as_path())?;

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
    crate::tracing::init_tracing(&log_file_path, log_level.as_deref())
        .map_err(|e| LuaError::RuntimeError(format!("Failed to initialize tracing: {}", e)))
}

fn create_exports(lua: &Lua) -> LuaResult<LuaTable> {
    let exports = lua.create_table()?;
    exports.set("init_db", lua.create_function(init_db)?)?;
    exports.set("destroy_db", lua.create_function(destroy_db)?)?;
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
    Ok(exports)
}

// https://github.com/mlua-rs/mlua/issues/318
#[mlua::lua_module(skip_memory_check)]
fn fff_nvim(lua: &Lua) -> LuaResult<LuaTable> {
    create_exports(lua)
}
