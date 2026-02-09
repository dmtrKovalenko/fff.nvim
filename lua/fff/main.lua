-- PERF: By default, this plugin initializes itself lazily,
-- so we do not require any modules at the top of this module.

local M = {}

M.state = { initialized = false }

--- Setup the file picker with the given configuration
--- @param config table Configuration options
function M.setup(config) vim.g.fff = config end

--- Find files in current directory
--- @param opts? table Optional configuration {renderer = custom_renderer}
function M.find_files(opts)
  local picker_ok, picker_ui = pcall(require, 'fff.picker_ui')
  if picker_ok then
    picker_ui.open(opts)
  else
    vim.notify('Failed to load picker UI: ' .. picker_ui, vim.log.levels.ERROR)
  end
end

function M.find_in_git_root()
  local git_root = vim.fn.system('git rev-parse --show-toplevel 2>/dev/null'):gsub('\n', '')
  if vim.v.shell_error ~= 0 then
    vim.notify('Not in a git repository', vim.log.levels.WARN)
    return
  end

  M.find_files_in_dir(git_root)
end

--- Trigger rescan of files in the current directory
function M.scan_files()
  local fuzzy = require('fff.core').ensure_initialized()
  local ok = pcall(fuzzy.scan_files)
  if not ok then vim.notify('Failed to scan files', vim.log.levels.ERROR) end
end

--- Refresh git status for the active file lock
function M.refresh_git_status()
  local fuzzy = require('fff.core').ensure_initialized()
  local ok, updated_files_count = pcall(fuzzy.refresh_git_status)
  if ok then
    vim.notify('Refreshed git status for ' .. tostring(updated_files_count) .. ' files', vim.log.levels.INFO)
  else
    vim.notify('Failed to refresh git status', vim.log.levels.ERROR)
  end
end

--- Search files programmatically
--- @param query string Search query
--- @param max_results number Maximum number of results
--- @return table List of matching files
function M.search(query, max_results)
  local fuzzy = require('fff.core').ensure_initialized()
  local config = require('fff.conf').get()
  max_results = max_results or config.max_results
  local max_threads = config.max_threads or 4
  local combo_boost_score_multiplier = config.history and config.history.combo_boost_score_multiplier or 100
  local min_combo_count = config.history and config.history.min_combo_count or 3
  -- Args: query, max_threads, current_file, combo_boost_score_multiplier, min_combo_count, offset, page_size
  local ok, search_result = pcall(
    fuzzy.fuzzy_search_files,
    query,
    max_threads,
    nil,
    combo_boost_score_multiplier,
    min_combo_count,
    0,
    max_results
  )
  if ok and search_result.items then return search_result.items end
  return {}
end

--- Search and show results in a nice format
--- @param query string Search query
function M.search_and_show(query)
  if not query or query == '' then
    M.find_files()
    return
  end

  local results = M.search(query, 20)

  if #results == 0 then
    print('🔍 No files found matching "' .. query .. '"')
    return
  end

  -- Filter out directories (should already be done by Rust, but just in case)
  local files = {}
  for _, item in ipairs(results) do
    if not item.is_dir then table.insert(files, item) end
  end

  if #files == 0 then
    print('🔍 No files found matching "' .. query .. '"')
    return
  end

  print('🔍 Found ' .. #files .. ' files matching "' .. query .. '":')

  for i, file in ipairs(files) do
    if i <= 15 then
      local icon = file.extension ~= '' and '.' .. file.extension or '📄'
      local frecency = file.frecency_score > 0 and ' ⭐' .. file.frecency_score or ''
      print('  ' .. i .. '. ' .. icon .. ' ' .. file.relative_path .. frecency)
    end
  end

  if #files > 15 then print('  ... and ' .. (#files - 15) .. ' more files') end

  print('Use :FFFFind to browse all files')
end

--- Get file preview
--- @param file_path string Path to the file
--- @return string|nil File content or nil if failed
function M.get_preview(file_path)
  local preview = require('fff.file_picker.preview')
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

--- Find files in a specific directory
--- @param directory string Directory path to search in
function M.find_files_in_dir(directory)
  if not directory then
    vim.notify('Directory path required for find_files_in_dir', vim.log.levels.ERROR)
    return
  end

  M.change_indexing_directory(directory)

  local picker_ok, picker_ui = pcall(require, 'fff.picker_ui')
  if picker_ok then
    picker_ui.open({ title = 'Files in ' .. vim.fn.fnamemodify(directory, ':t') })
  else
    vim.notify('Failed to load picker UI', vim.log.levels.ERROR)
  end
end

--- Change the base directory for the file picker
--- @param new_path string New directory path to use as base
--- @return boolean `true` if successful, `false` otherwise
function M.change_indexing_directory(new_path)
  if not new_path or new_path == '' then
    vim.notify('Directory path is required', vim.log.levels.ERROR)
    return false
  end

  local expanded_path = vim.fn.expand(new_path)

  if vim.fn.isdirectory(expanded_path) ~= 1 then
    vim.notify('Directory does not exist: ' .. expanded_path, vim.log.levels.ERROR)
    return false
  end

  local fuzzy = require('fff.core').ensure_initialized()
  local ok, result = pcall(fuzzy.restart_index_in_path, expanded_path)
  if not ok then
    vim.notify('Failed to change directory: ' .. result, vim.log.levels.ERROR)
    return false
  end

  local config = require('fff.conf').get()
  config.base_path = expanded_path
  return true
end

--- Opens the file under the cursor with an optional callback if the only file
--- is found and we are about to inline open it
--- @param open_cb function|nil Optional callback function to execute after opening the file
function M.open_file_under_cursor(open_cb)
  local filename = vim.fn.expand('<cfile>')
  local full_path_with_suffix = vim.fn.expand('<cWORD>')

  local picker_ok, picker_ui = pcall(require, 'fff.picker_ui')
  if not picker_ok then
    vim.notify('Failed to load picker UI', vim.log.levels.ERROR)
    return
  end

  picker_ui.open_with_callback(full_path_with_suffix, function(files, metadata, location, get_file_score)
    if #files == 1 or require('fff.file_picker').get_file_score(1).exact_match then
      if open_cb and type(open_cb) == 'function' then open_cb(files[1].path) end
      vim.api.nvim_command(string.format('e %s', vim.fn.fnameescape(files[1].path)))

      if location then vim.schedule(function() require('fff.location_utils').jump_to_location(location) end) end

      return true
    else
      return false -- Open UI with results
    end
  end)
end

return M
