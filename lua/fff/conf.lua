local M = {}

---@class fff.conf.State
local state = {
  ---@type table | nil
  config = nil,
}

local DEPRECATION_RULES = {
  {
    -- Top-level width -> layout.width
    old_path = { 'width' },
    new_path = { 'layout', 'width' },
    message = 'config.width is deprecated. Use config.layout.width instead.',
  },
  {
    -- Top-level height -> layout.height
    old_path = { 'height' },
    new_path = { 'layout', 'height' },
    message = 'config.height is deprecated. Use config.layout.height instead.',
  },
  {
    -- preview.width -> layout.preview_size
    old_path = { 'preview', 'width' },
    new_path = { 'layout', 'preview_size' },
    message = 'config.preview.width is deprecated. Use config.layout.preview_size instead.',
  },
  {
    -- layout.preview_width -> layout.preview_size
    old_path = { 'layout', 'preview_width' },
    new_path = { 'layout', 'preview_size' },
    message = 'config.layout.preview_width is deprecated. Use config.layout.preview_size instead.',
  },
}

--- Get value from nested table using path array
--- @param tbl table Source table
--- @param path table Array of keys to traverse
--- @return any|nil Value at path or nil if not found
local function get_nested_value(tbl, path)
  local current = tbl
  for _, key in ipairs(path) do
    if type(current) ~= 'table' or current[key] == nil then return nil end
    current = current[key]
  end

  return current
end

--- Set value in nested table using path array, creating intermediate tables
--- @param tbl table Target table
--- @param path table Array of keys to traverse
--- @param value any Value to set
local function set_nested_value(tbl, path, value)
  local current = tbl
  for i = 1, #path - 1 do
    local key = path[i]
    if type(current[key]) ~= 'table' then current[key] = {} end
    current = current[key]
  end

  current[path[#path]] = value
end

--- Remove value from nested table using path array
--- @param tbl table Target table
--- @param path table Array of keys to traverse
local function remove_nested_value(tbl, path)
  if #path == 0 then return end

  local current = tbl
  for i = 1, #path - 1 do
    local key = path[i]
    if type(current[key]) ~= 'table' then return end
    current = current[key]
  end

  current[path[#path]] = nil
end

--- Handle deprecated configuration options with migration warnings
--- @param user_config table User provided configuration
--- @return table Migrated configuration
local function handle_deprecated_config(user_config)
  if not user_config then return {} end

  local migrated_config = vim.deepcopy(user_config)

  for _, rule in ipairs(DEPRECATION_RULES) do
    local old_value = get_nested_value(user_config, rule.old_path)
    if old_value ~= nil then
      set_nested_value(migrated_config, rule.new_path, old_value)
      remove_nested_value(migrated_config, rule.old_path)

      vim.notify('FFF: ' .. rule.message, vim.log.levels.WARN)
    end
  end

  return migrated_config
end

local function init()
  local config = vim.g.fff or {}
  local default_config = {
    base_path = vim.fn.getcwd(),
    prompt = '🪿 ',
    title = 'FFFiles',
    max_results = 100,
    max_threads = 4,
    layout = {
      height = 0.8,
      width = 0.8,
      prompt_position = 'bottom', -- or 'top'
      preview_position = 'right', -- or 'left', 'right', 'top', 'bottom'
      preview_size = 0.5,
    },
    preview = {
      enabled = true,
      max_size = 10 * 1024 * 1024, -- Do not try to read files larger than 10MB
      chunk_size = 8192, -- Bytes per chunk for dynamic loading (8kb - fits ~100-200 lines)
      binary_file_threshold = 1024, -- amount of bytes to scan for binary content (set 0 to disable)
      imagemagick_info_format_str = '%m: %wx%h, %[colorspace], %q-bit',
      line_numbers = false,
      wrap_lines = false,
      show_file_info = true,
      filetypes = {
        svg = { wrap_lines = true },
        markdown = { wrap_lines = true },
        text = { wrap_lines = true },
      },
    },
    keymaps = {
      close = '<Esc>',
      select = '<CR>',
      select_split = '<C-s>',
      select_vsplit = '<C-v>',
      select_tab = '<C-t>',
      move_up = { '<Up>', '<C-p>' },
      move_down = { '<Down>', '<C-n>' },
      preview_scroll_up = '<C-u>',
      preview_scroll_down = '<C-d>',
      toggle_debug = '<F2>',
    },
    hl = {
      border = 'FloatBorder',
      normal = 'Normal',
      cursor = 'CursorLine',
      matched = 'IncSearch',
      title = 'Title',
      prompt = 'Question',
      active_file = 'Visual',
      frecency = 'Number',
      debug = 'Comment',
    },
    frecency = {
      enabled = true,
      db_path = vim.fn.stdpath('cache') .. '/fff_nvim',
    },
    debug = {
      enabled = false, -- Set to true to show scores in the UI
      show_scores = false,
    },
    logging = {
      enabled = true,
      log_file = vim.fn.stdpath('log') .. '/fff.log',
      log_level = 'info',
    },
  }

  local migrated_user_config = handle_deprecated_config(config)
  local merged_config = vim.tbl_deep_extend('force', default_config, migrated_user_config)

  state.config = merged_config
end

--- Setup the file picker with the given configuration
--- @param config table Configuration options
function M.setup(config) vim.g.fff = config end

--- @return table the fff configuration
function M.get()
  if not state.config then init() end
  return state.config
end

--- @return boolean state_changed
function M.toggle_debug()
  local old_debug_state = state.config.debug.show_scores
  state.config.debug.show_scores = not state.config.debug.show_scores
  local status = state.config.debug.show_scores and 'enabled' or 'disabled'
  vim.notify('FFF debug scores ' .. status, vim.log.levels.INFO)
  return old_debug_state ~= state.config.debug.show_scores
end

return M
