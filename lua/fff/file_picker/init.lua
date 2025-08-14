--- FFF.nvim File Picker - High-performance file picker for Neovim
--- Uses advanced fuzzy search algorithm with frecency scoring
---@module "fff"

local M = {}

local state = require('fff.state')
-- Load the fuzzy module for file operations
local fuzzy = require('fff.fuzzy')

--- Initialize the file picker
--- @param config table Configuration for the file picker
function M.setup(config)
  state.config = vim.tbl_deep_extend('force', state.config, config or {})

  local ok, result = pcall(fuzzy.init_db, state.config.frecency.db_path, true)
  if not ok then vim.notify('Failed to initialize frecency database: ' .. result, vim.log.levels.WARN) end

  ok, result = pcall(fuzzy.init_file_picker, state.config.base_path)
  if not ok then
    vim.notify('Failed to initialize file picker: ' .. result, vim.log.levels.ERROR)
    return false
  end

  state.initialized = true

  return true
end

--- Trigger scan of files in the current directory (asynchronous)
function M.scan_files()
  if not state.initialized then return end

  local ok, result = pcall(fuzzy.scan_files)
  if not ok then
    vim.notify('Failed to trigger file scan: ' .. result, vim.log.levels.ERROR)
    return
  end

  state.last_scan_time = os.time()
end

---@class FileItem
---@field path string
---@field relative_path string
---@field file_name string
---@field size integer
---@field modified number
---@field access_frecency_score number
---@field modification_frecency_score number
---@field total_frecency_score number
---@field git_status number?

---@class Scores
---@field total number
---@field base_score number
---@field filename_bonus number
---@field special_filename_bonus number
---@field frecency_boost number
---@field distance_penalty number
---@field match_type number

---@class SearchResult fuzzy search result from rust
---@field items FileItem[]  # list of files
---@field scores Scores[]   # list of match scores
---@field total_matched integer
---@field total_files integer

--- Search files with fuzzy matching using blink.cmp's advanced algorithm
--- @param query string Search query
--- @param max_results number Maximum number of results (optional)
--- @param current_file string? Path to current file to deprioritize (optional)
--- @return table List of matching files
function M.search_files(query, max_results, max_threads, current_file)
  if not state.initialized then return {} end

  max_results = max_results or state.config.max_results
  max_threads = max_threads or state.config.max_threads

  ---@return boolean, SearchResult
  local ok, search_result = pcall(fuzzy.fuzzy_search_files, query, max_results, max_threads, current_file)
  if not ok then
    vim.notify('Failed to search files: ' .. tostring(search_result), vim.log.levels.ERROR)
    return {}
  end

  -- Store search metadata for UI display
  state.last_search_result = search_result

  return search_result.items
end

--- Get the last search result metadata
--- @return table Search metadata with total_matched and total_files
function M.get_search_metadata()
  if not state.last_search_result then return { total_matched = 0, total_files = 0 } end
  return {
    total_matched = state.last_search_result.total_matched,
    total_files = state.last_search_result.total_files,
  }
end

--- Get score information for a file by index (1-based)
--- @param index number The index of the file in the last search results
--- @return table|nil Score information or nil if not available
function M.get_file_score(index)
  if not state.last_search_result or not state.last_search_result.scores then return nil end

  -- Convert to 0-based index for Lua table access
  local score = state.last_search_result.scores[index]
  if not score then return nil end

  return {
    total = score.total or 0,
    base_score = score.base_score or 0,
    filename_bonus = score.filename_bonus or 0,
    special_filename_bonus = score.special_filename_bonus or 0,
    frecency_boost = score.frecency_boost or 0,
    distance_penalty = score.distance_penalty or 0,
    match_type = score.match_type or 'unknown',
  }
end

--- Record file access for frecency tracking
--- @param file_path string Path to the file that was accessed
function M.track_access(file_path)
  if not state.initialized then return end

  local ok, result = pcall(fuzzy.track_access, file_path)
  if not ok then vim.notify('Failed to record file access: ' .. result, vim.log.levels.WARN) end
end

--- Get file content for preview
--- @param file_path string Path to the file
--- @return string|nil File content or nil if failed
function M.get_file_preview(file_path)
  local preview = require('fff.file_picker.preview')

  -- Create a temporary buffer to get the preview
  local temp_buf = vim.api.nvim_create_buf(false, true)
  local success = preview.preview(file_path, temp_buf)

  if not success then
    vim.api.nvim_buf_delete(temp_buf, { force = true })
    return nil
  end

  local lines = vim.api.nvim_buf_get_lines(temp_buf, 0, -1, false)
  vim.api.nvim_buf_delete(temp_buf, { force = true })

  return table.concat(lines, '\n')
end

--- Check if file picker is initialized
--- @return boolean
function M.is_initialized() return state.initialized end

--- Get scan progress information
--- @return table Progress information with scanned_files_count, is_scanning
function M.get_scan_progress()
  if not state.initialized then return { total_files = 0, scanned_files_count = 0, is_scanning = false } end

  local ok, result = pcall(fuzzy.get_scan_progress)
  if not ok then
    vim.notify('Failed to get scan progress: ' .. result, vim.log.levels.WARN)
    return { scanned_files_count = 0, is_scanning = false }
  end

  return result
end

--- Refresh git status on cached files (call after git status loading completes)
--- @return table List of files with updated git status
function M.refresh_git_status()
  if not state.initialized then return {} end

  local ok, result = pcall(fuzzy.refresh_git_status)
  if not ok then
    vim.notify('Failed to refresh git status: ' .. result, vim.log.levels.WARN)
    return {}
  end

  -- Update our cache
  return result
end

--- Stop background git status monitoring
--- @return boolean Success status
function M.stop_background_monitor()
  if not state.initialized then return false end

  local ok, result = pcall(fuzzy.stop_background_monitor)
  if not ok then
    vim.notify('Failed to stop background monitor: ' .. result, vim.log.levels.WARN)
    return false
  end
  return result
end

--- Wait for initial scan to complete
--- @param timeout_ms number Optional timeout in milliseconds (default 5000)
--- @return boolean True if scan completed, false if timed out
function M.wait_for_initial_scan(timeout_ms)
  if not state.initialized then return false end

  local ok, result = pcall(fuzzy.wait_for_initial_scan, timeout_ms)
  if not ok then
    vim.notify('Failed to wait for initial scan: ' .. result, vim.log.levels.WARN)
    return false
  end
  return result
end

return M
