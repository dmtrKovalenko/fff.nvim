use crate::error::Error;
use crate::file_key::FileKey;
use crate::file_picker::FilePicker;
use crate::frecency::FrecencyTracker;
use crate::types::{FileItem, SearchResult};
use mlua::prelude::*;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

mod error;
mod file_key;
mod file_picker;
mod frecency;
mod git;
mod path_utils;
pub(crate) mod score;
mod tracing;
pub(crate) mod types;

static FRECENCY: LazyLock<RwLock<Option<FrecencyTracker>>> = LazyLock::new(|| RwLock::new(None));
static FILE_PICKER: LazyLock<RwLock<Option<FilePicker>>> = LazyLock::new(|| RwLock::new(None));

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

pub fn scan_files(_: &Lua, _: ()) -> LuaResult<()> {
    let file_picker = FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)?;
    let picker = file_picker
        .as_ref()
        .ok_or_else(|| Error::InvalidPath("File picker not initialized".to_string()))?;
    let result = picker.trigger_rescan()?;
    ::tracing::info!("scan_files trigger_rescan completed");
    Ok(result)
}

pub fn get_cached_files(_: &Lua, _: ()) -> LuaResult<Vec<FileItem>> {
    let file_picker = FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)?;
    let picker = file_picker
        .as_ref()
        .ok_or_else(|| Error::InvalidPath("File picker not initialized".to_string()))?;
    Ok(picker.get_cached_files())
}

pub fn fuzzy_search_files(
    _: &Lua,
    (query, max_results, max_threads, current_file): (String, usize, usize, Option<String>),
) -> LuaResult<SearchResult> {
    let time = std::time::Instant::now();
    let file_picker = FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)?;
    ::tracing::debug!("Fuzzy search started: {:?}", time.elapsed());
    let picker = file_picker
        .as_ref()
        .ok_or_else(|| Error::InvalidPath("File picker not initialized".to_string()))?;

    let results = picker.fuzzy_search(&query, max_results, max_threads, current_file);
    Ok(results)
}

pub fn access_file(_: &Lua, file_path: String) -> LuaResult<bool> {
    let frecency = FRECENCY.read().map_err(|_| Error::AcquireFrecencyLock)?;
    if let Some(ref tracker) = *frecency {
        let file_key = FileKey { path: file_path };
        tracker.track_access(&file_key)?;
    }
    Ok(true)
}

pub fn get_scan_progress(lua: &Lua, _: ()) -> LuaResult<LuaValue> {
    let file_picker = FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)?;
    let picker = file_picker
        .as_ref()
        .ok_or_else(|| Error::InvalidPath("File picker not initialized".to_string()))?;
    let progress = picker.get_scan_progress();

    let table = lua.create_table()?;
    table.set("total_files", progress.total_files)?;
    table.set("scanned_files", progress.scanned_files)?;
    table.set("is_scanning", progress.is_scanning)?;
    Ok(LuaValue::Table(table))
}

pub fn is_scanning(_: &Lua, _: ()) -> LuaResult<bool> {
    let file_picker = FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)?;
    let picker = file_picker
        .as_ref()
        .ok_or_else(|| Error::InvalidPath("File picker not initialized".to_string()))?;
    Ok(picker.is_scan_active())
}

pub fn refresh_git_status(_: &Lua, _: ()) -> LuaResult<Vec<FileItem>> {
    let file_picker = FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)?;
    let picker = file_picker
        .as_ref()
        .ok_or_else(|| Error::InvalidPath("File picker not initialized".to_string()))?;
    Ok(picker.refresh_git_status()?)
}

pub fn stop_background_monitor(_: &Lua, _: ()) -> LuaResult<bool> {
    let file_picker = FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)?;
    let picker = file_picker
        .as_ref()
        .ok_or_else(|| Error::InvalidPath("File picker not initialized".to_string()))?;
    picker.stop_background_monitor();
    Ok(true)
}

pub fn cancel_scan(_: &Lua, _: ()) -> LuaResult<bool> {
    Ok(true)
}

pub fn wait_for_initial_scan(_: &Lua, timeout_ms: Option<u64>) -> LuaResult<bool> {
    let file_picker = FILE_PICKER.read().map_err(|_| Error::AcquireItemLock)?;
    let picker = file_picker
        .as_ref()
        .ok_or_else(|| Error::InvalidPath("File picker not initialized".to_string()))?;

    let timeout = Duration::from_millis(timeout_ms.unwrap_or(5000)); // Default 5s timeout
    let start_time = std::time::Instant::now();

    while picker.is_scan_active() && start_time.elapsed() < timeout {
        std::thread::sleep(Duration::from_millis(50));
    }

    Ok(!picker.is_scan_active())
}

pub fn init_tracing(
    _: &Lua,
    (log_file_path, log_level): (String, Option<String>),
) -> LuaResult<String> {
    let level = log_level.unwrap_or_else(|| "info".to_string());
    crate::tracing::init_tracing(&log_file_path, &level)
        .map_err(|e| LuaError::RuntimeError(format!("Failed to initialize tracing: {}", e)))
}

fn create_exports(lua: &Lua) -> LuaResult<LuaTable> {
    let exports = lua.create_table()?;
    exports.set("init_db", lua.create_function(init_db)?)?;
    exports.set("destroy_db", lua.create_function(destroy_db)?)?;
    exports.set("init_file_picker", lua.create_function(init_file_picker)?)?;
    exports.set("scan_files", lua.create_function(scan_files)?)?;
    exports.set("get_cached_files", lua.create_function(get_cached_files)?)?;
    exports.set(
        "fuzzy_search_files",
        lua.create_function(fuzzy_search_files)?,
    )?;
    exports.set("access_file", lua.create_function(access_file)?)?;
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
    Ok(exports)
}

// https://github.com/mlua-rs/mlua/issues/318
#[mlua::lua_module(skip_memory_check)]
fn fff_nvim(lua: &Lua) -> LuaResult<LuaTable> {
    create_exports(lua)
}
