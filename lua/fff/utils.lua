local M = {}

--- Format file size into human-readable string
--- @param size number File size in bytes
--- @return string Formatted size string (e.g., "1.2 KB", "3.4 MB")
function M.format_file_size(size)
  if not size or size < 0 then return 'Unknown' end

  if size < 1024 then
    return string.format('%d B', size)
  elseif size < 1024 * 1024 then
    return string.format('%.1f KB', size / 1024)
  elseif size < 1024 * 1024 * 1024 then
    return string.format('%.1f MB', size / (1024 * 1024))
  else
    return string.format('%.1f GB', size / (1024 * 1024 * 1024))
  end
end

local function get_fixed_filetype_detection(extension)
  local extension_map = {
    ts = 'typescript',
    tex = 'latex',
    md = 'markdown',
    txt = 'text',
  }

  return extension_map[extension]
end

--- Detect filetype with various fallbacks
--- @param file_path string the filetype
--- @return string detected filetype
function M.detect_filetype(file_path)
  local has_plenary, plenary_filetype = pcall(require, 'plenary.filetype')
  if has_plenary then
    local detected = plenary_filetype.detect(file_path, {})
    if detected and detected ~= '' then return detected end
  end

  local builtin_filetype = vim.filetype.match({ filename = file_path })
  if builtin_filetype and builtin_filetype ~= '' then return builtin_filetype end

  local extension = vim.fn.fnamemodify(file_path, ':e'):lower()
  return get_fixed_filetype_detection(extension)
end

--- Safely resolve a config value that can be either a static value or a function
--- @param config_value any The config value (can be function or static value)
--- @param terminal_width number Terminal width for function calls
--- @param terminal_height number Terminal height for function calls
--- @param validator function Function to validate the result
--- @param fallback any Fallback value if function fails or returns invalid value
--- @param error_context string Context for error messages
--- @return number The resolved and validated value
function M.resolve_config_value(config_value, terminal_width, terminal_height, validator, fallback, error_context)
  if type(config_value) == 'function' then
    local success, result = pcall(config_value, terminal_width, terminal_height)

    if success and validator(result) then
      return result
    else
      if not success then
        vim.notify('FFF: Error in ' .. error_context .. ' function: ' .. tostring(result), vim.log.levels.WARN)
      end
      return fallback
    end
  else
    if config_value == nil or not validator(config_value) then return fallback end
    return config_value
  end
end

--- Validate numeric ratio (0 < value <= 1)
--- @param value any Value to validate
--- @return boolean True if valid numeric ratio
function M.is_valid_ratio(value) return type(value) == 'number' and value > 0 and value <= 1 end

--- Validate position string
--- @param value any Value to validate
--- @param values table List of valid values strings
--- @return boolean True if valid position
function M.is_one_of(value, values)
  if type(value) ~= 'string' then return false end
  for _, pos in ipairs(values) do
    if value == pos then return true end
  end
  return false
end

--- Resolve an indexer-relative path to an absolute one against the picker's current `base_path`.
--- @param relative_path string|nil
--- @return string|nil
function M.canonicalize_fff_path(relative_path)
  if not relative_path or relative_path == '' then return nil end
  local path = relative_path
  -- Strip Windows long-path prefix (\\?\) — Neovim cannot open these.
  if vim.startswith(path, '\\\\?\\') then path = path:sub(5) end
  if vim.fn.fnamemodify(path, ':p') == path then return path end
  local base = require('fff.conf').get().base_path
  if not base or base == '' then return path end
  return vim.fs.normalize(base .. '/' .. path)
end

--- Whether a window has `winfixbuf` set (cannot host a different buffer).
--- @param win number Window ID
--- @return boolean
function M.window_has_winfixbuf(win)
  local ok, val = pcall(vim.api.nvim_get_option_value, 'winfixbuf', { win = win })
  return ok and val == true
end

--- Find the first window in the current tabpage that can host a regular file
--- buffer (writable, not locked, not the picker's own floats).
--- @param exclude_wins? table<number, boolean> Optional set of window IDs to skip.
--- @return number|nil
function M.find_suitable_window(exclude_wins)
  exclude_wins = exclude_wins or {}
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(vim.api.nvim_get_current_tabpage())) do
    if vim.api.nvim_win_is_valid(win) and not exclude_wins[win] then
      local buf = vim.api.nvim_win_get_buf(win)
      if vim.api.nvim_buf_is_valid(buf) then
        local buftype = vim.api.nvim_get_option_value('buftype', { buf = buf })
        local modifiable = vim.api.nvim_get_option_value('modifiable', { buf = buf })
        local filetype = vim.api.nvim_get_option_value('filetype', { buf = buf })
        if
          (buftype == '' or buftype == 'acwrite')
          and modifiable
          and filetype ~= 'undotree'
          and not M.window_has_winfixbuf(win)
        then
          return win
        end
      end
    end
  end
  return nil
end

return M
