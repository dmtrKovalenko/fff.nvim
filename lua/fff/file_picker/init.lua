--- FFF.nvim File Picker - High-performance file picker for Neovim
--- Uses advanced fuzzy search algorithm with frecency scoring

local M = {}

-- Load the fuzzy module for file operations
local fuzzy = require('fff.fuzzy')

-- State
M.state = {
  initialized = false,
  base_path = nil,
  last_scan_time = 0,
  config = nil,
}

--- Initialize the file picker
--- @param config table Configuration for the file picker
function M.setup(config)
  config = config or {}

  -- Default configuration
  local defaults = {
    base_path = vim.fn.getcwd(),
    max_results = 100,
    max_threads = 4,
    show_hidden = false,
    ignore_patterns = {},
    preview = {
      enabled = true,
      max_lines = 100,
      max_size = 1024 * 1024, -- 1MB
    },
    keymaps = {
      select = '<CR>',
      vsplit = '<C-v>',
      split = '<C-s>',
      tab = '<C-t>',
      close = '<Esc>',
      preview_up = '<C-u>',
      preview_down = '<C-d>',
    },
    layout = {
      prompt_position = 'bottom',
      preview_position = 'right',
      preview_size = 0.4,
      height = 0.8,
      width = 0.8,
    },
  }

  M.config = vim.tbl_deep_extend('force', defaults, config)
  M.state.config = M.config

  local db_path = vim.fn.stdpath('cache') .. '/fff_nvim'
  local ok, result = pcall(fuzzy.init_db, db_path, true)
  if not ok then vim.notify('Failed to initialize frecency database: ' .. result, vim.log.levels.WARN) end

  ok, result = pcall(fuzzy.init_file_picker, M.config.base_path)
  if not ok then
    vim.notify('Failed to initialize file picker: ' .. result, vim.log.levels.ERROR)
    return false
  end

  M.state.initialized = true
  M.state.base_path = M.config.base_path

  return true
end

--- Trigger scan of files in the current directory (asynchronous)
function M.scan_files()
  if not M.state.initialized then return end

  local ok, result = pcall(fuzzy.scan_files)
  if not ok then
    vim.notify('Failed to trigger file scan: ' .. result, vim.log.levels.ERROR)
    return
  end

  M.state.last_scan_time = os.time()
end

--- Search files with fuzzy matching using blink.cmp's advanced algorithm
--- @param query string Search query
--- @param max_results number Maximum number of results (optional)
--- @param max_threads number Maximum number of threads (optional)
--- @param current_file string|nil Path to current file to deprioritize (optional)
--- @param reverse_order boolean Reverse order of results
--- @return table List of matching files
function M.search_files(query, max_results, max_threads, current_file, reverse_order)
  if not M.state.initialized then return {} end

  max_results = max_results or M.config.max_results
  max_threads = max_threads or M.config.max_threads

  local ok, search_result =
    pcall(fuzzy.fuzzy_search_files, query, max_results, max_threads, current_file, reverse_order)
  if not ok then
    vim.notify('Failed to search files: ' .. tostring(search_result), vim.log.levels.ERROR)
    return {}
  end

  -- Store search metadata for UI display
  M.state.last_search_result = search_result

  return search_result.items
end

--- Get the last search result metadata
--- @return table Search metadata with total_matched and total_files
function M.get_search_metadata()
  if not M.state.last_search_result then return { total_matched = 0, total_files = 0 } end
  return {
    total_matched = M.state.last_search_result.total_matched,
    total_files = M.state.last_search_result.total_files,
  }
end

--- Get score information for a file by index (1-based)
--- @param index number The index of the file in the last search results
--- @return table|nil Score information or nil if not available
function M.get_file_score(index)
  if not M.state.last_search_result or not M.state.last_search_result.scores then return nil end

  -- Convert to 0-based index for Lua table access
  local score = M.state.last_search_result.scores[index]
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
  if not M.state.initialized then return end

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
function M.is_initialized() return M.state.initialized end

--- Get current configuration
--- @return table
function M.get_config() return M.config end

--- Get scan progress information
--- @return table Progress information with scanned_files_count, is_scanning
function M.get_scan_progress()
  if not M.state.initialized then return { total_files = 0, scanned_files_count = 0, is_scanning = false } end

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
  if not M.state.initialized then return {} end

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
  if not M.state.initialized then return false end

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
  if not M.state.initialized then return false end

  local ok, result = pcall(fuzzy.wait_for_initial_scan, timeout_ms)
  if not ok then
    vim.notify('Failed to wait for initial scan: ' .. result, vim.log.levels.WARN)
    return false
  end
  return result
end

--- Get current state
--- @return table
function M.get_state() return M.state end

return M
