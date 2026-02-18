local M = {}

local conf = require('fff.conf')
local file_picker = require('fff.file_picker')
local preview = require('fff.file_picker.preview')
local utils = require('fff.utils')
local location_utils = require('fff.location_utils')
local combo_renderer = require('fff.combo_renderer')
local list_renderer = require('fff.list_renderer')
local scrollbar = require('fff.scrollbar')
local rust = require('fff.rust')

local BORDER_PRESETS = {
  single = { '┌', '─', '┐', '│', '┘', '─', '└', '│' },
  double = { '╔', '═', '╗', '║', '╝', '═', '╚', '║' },
  rounded = { '╭', '─', '╮', '│', '╯', '─', '╰', '│' },
  solid = { '▛', '▀', '▜', '▐', '▟', '▄', '▙', '▌' },
  shadow = { '', '', ' ', ' ', ' ', ' ', ' ', '' },
  none = { '', '', '', '', '', '', '', '' },
}

local T_JUNCTION_PRESETS = {
  single = { '├', '┤' },
  double = { '╠', '╣' },
  rounded = { '├', '┤' }, -- Rounded only affects corners
  solid = { '▌', '▐' },
  shadow = { '', '' },
  none = { '', '' },
}

--- Get border characters from vim.o.winborder for custom connected borders
--- @return table Array of 8 border characters
--- @return table Array of 2 T-junction characters (left, right)
local function get_border_chars()
  local winborder = vim.o.winborder or 'single'

  if BORDER_PRESETS[winborder] then return BORDER_PRESETS[winborder], T_JUNCTION_PRESETS[winborder] end

  -- Fallback to single for unknown border styles
  return BORDER_PRESETS.single, T_JUNCTION_PRESETS.single
end

local function get_prompt_position()
  local config = M.state.config

  if config and config.layout and config.layout.prompt_position then
    local terminal_width = vim.o.columns
    local terminal_height = vim.o.lines

    return utils.resolve_config_value(
      config.layout.prompt_position,
      terminal_width,
      terminal_height,
      function(value) return utils.is_one_of(value, { 'top', 'bottom' }) end,
      'bottom',
      'layout.prompt_position'
    )
  end

  return 'bottom'
end

local function get_preview_position()
  local config = M.state.config

  if config and config.layout and config.layout.preview_position then
    local terminal_width = vim.o.columns
    local terminal_height = vim.o.lines

    return utils.resolve_config_value(
      config.layout.preview_position,
      terminal_width,
      terminal_height,
      function(value) return utils.is_one_of(value, { 'left', 'right', 'top', 'bottom' }) end,
      'right',
      'layout.preview_position'
    )
  end

  return 'right'
end

--- Function-based config options:
--- config.layout.width: number|function(terminal_width, terminal_height): number
--- config.layout.height: number|function(terminal_width, terminal_height): number
--- config.layout.preview_size: number|function(terminal_width, terminal_height): number
--- config.layout.preview_position: string|function(terminal_width, terminal_height): string
--- config.layout.prompt_position: string|function(terminal_width, terminal_height): string

--- @class LayoutConfig
--- @field total_width number
--- @field total_height number
--- @field start_col number
--- @field start_row number
--- @field preview_position string|function Preview position ('left'|'right'|'top'|'bottom') or function(terminal_width, terminal_height): string
--- @field prompt_position string
--- @field debug_enabled boolean
--- @field preview_width number
--- @field preview_height number
--- @field separator_width number
--- @field file_info_height number

--- Calculate layout dimensions and positions for all windows
--- @param cfg LayoutConfig
--- @return table Layout configuration
function M.calculate_layout_dimensions(cfg)
  local BORDER_SIZE = 2
  local PROMPT_HEIGHT = 2
  local SEPARATOR_WIDTH = 1
  local SEPARATOR_HEIGHT = 1

  if not utils.is_one_of(cfg.preview_position, { 'left', 'right', 'top', 'bottom' }) then
    error('Invalid preview position: ' .. tostring(cfg.preview_position))
  end

  local layout = {}
  local preview_enabled = M.enabled_preview()

  -- Section 1: Base dimensions and bounds checking
  local total_width = math.max(0, cfg.total_width - BORDER_SIZE)
  local total_height = math.max(0, cfg.total_height - BORDER_SIZE - PROMPT_HEIGHT)

  -- Section 2: Calculate dimensions based on preview position
  if cfg.preview_position == 'left' then
    local separator_width = preview_enabled and SEPARATOR_WIDTH or 0
    local list_width = math.max(0, total_width - cfg.preview_width - separator_width)
    local list_height = total_height

    layout.list_col = cfg.start_col + cfg.preview_width + 3 -- +3 for borders and separator
    layout.list_width = list_width
    layout.list_height = list_height
    layout.input_col = layout.list_col
    layout.input_width = list_width

    if preview_enabled then
      layout.preview = {
        col = cfg.start_col + 1,
        row = cfg.start_row + 1,
        width = cfg.preview_width,
        height = list_height,
      }
    end
  elseif cfg.preview_position == 'right' then
    local separator_width = preview_enabled and SEPARATOR_WIDTH or 0
    local list_width = math.max(0, total_width - cfg.preview_width - separator_width)
    local list_height = total_height

    layout.list_col = cfg.start_col + 1
    layout.list_width = list_width
    layout.list_height = list_height
    layout.input_col = layout.list_col
    layout.input_width = list_width

    if preview_enabled then
      layout.preview = {
        col = cfg.start_col + list_width + 3, -- +3 for borders and separator (matches original)
        row = cfg.start_row + 1,
        width = cfg.preview_width,
        height = list_height,
      }
    end
  elseif cfg.preview_position == 'top' then
    local separator_height = preview_enabled and SEPARATOR_HEIGHT or 0
    local list_height = math.max(0, total_height - cfg.preview_height - separator_height)

    layout.list_col = cfg.start_col + 1
    layout.list_width = total_width
    layout.list_height = list_height
    layout.input_col = layout.list_col
    layout.input_width = total_width
    layout.list_start_row = cfg.start_row + (preview_enabled and (cfg.preview_height + separator_height) or 0) + 1

    if preview_enabled then
      layout.preview = {
        col = cfg.start_col + 1,
        row = cfg.start_row + 1,
        width = total_width,
        height = cfg.preview_height,
      }
    end
  else
    local separator_height = preview_enabled and SEPARATOR_HEIGHT or 0
    local list_height = math.max(0, total_height - cfg.preview_height - separator_height)

    layout.list_col = cfg.start_col + 1
    layout.list_width = total_width
    layout.list_height = list_height
    layout.input_col = layout.list_col
    layout.input_width = total_width
    layout.list_start_row = cfg.start_row + 1

    if preview_enabled then
      layout.preview = {
        col = cfg.start_col + 1,
        width = total_width,
        height = cfg.preview_height,
      }
    end
  end

  -- Section 3: Position prompt and adjust row positions
  if cfg.preview_position == 'left' or cfg.preview_position == 'right' then
    if cfg.prompt_position == 'top' then
      layout.input_row = cfg.start_row + 1
      layout.list_row = cfg.start_row + PROMPT_HEIGHT + 1
    else
      layout.list_row = cfg.start_row + 1
      layout.input_row = cfg.start_row + cfg.total_height - BORDER_SIZE
    end

    if layout.preview then
      if cfg.prompt_position == 'top' then
        layout.preview.row = cfg.start_row + 1
        layout.preview.height = cfg.total_height - BORDER_SIZE
      else
        layout.preview.row = cfg.start_row + 1
        layout.preview.height = cfg.total_height - BORDER_SIZE
      end
    end
  else
    local list_start_row = layout.list_start_row
    if cfg.prompt_position == 'top' then
      layout.input_row = list_start_row
      layout.list_row = list_start_row + BORDER_SIZE
      layout.list_height = math.max(0, layout.list_height - BORDER_SIZE)
    else
      layout.list_row = list_start_row
      layout.input_row = list_start_row + layout.list_height + 1
    end

    if cfg.preview_position == 'bottom' and layout.preview then
      if cfg.prompt_position == 'top' then
        layout.preview.row = layout.list_row + layout.list_height + 1
      else
        layout.preview.row = layout.input_row + PROMPT_HEIGHT
      end
    end
  end

  -- Section 4: Position debug panel (if enabled)
  if cfg.debug_enabled and preview_enabled and layout.preview then
    if cfg.preview_position == 'left' or cfg.preview_position == 'right' then
      layout.file_info = {
        width = layout.preview.width,
        height = cfg.file_info_height,
        col = layout.preview.col,
        row = layout.preview.row,
      }
      layout.preview.row = layout.preview.row + cfg.file_info_height + SEPARATOR_HEIGHT + 1
      layout.preview.height = math.max(3, layout.preview.height - cfg.file_info_height - SEPARATOR_HEIGHT - 1)
    else
      layout.file_info = {
        width = layout.preview.width,
        height = cfg.file_info_height,
        col = layout.preview.col,
        row = layout.preview.row,
      }
      layout.preview.row = layout.preview.row + cfg.file_info_height + SEPARATOR_HEIGHT + 1
      layout.preview.height = math.max(3, layout.preview.height - cfg.file_info_height - SEPARATOR_HEIGHT - 1)
    end
  end

  return layout
end

local preview_config = conf.get().preview
if preview_config then preview.setup(preview_config) end

M.state = {
  active = false,
  layout = nil,
  input_win = nil,
  input_buf = nil,
  list_win = nil,
  list_buf = nil,
  file_info_win = nil,
  file_info_buf = nil,
  preview_win = nil,
  preview_buf = nil,

  items = {},
  filtered_items = {},
  cursor = 1,
  top = 1,
  query = '',
  item_line_map = {},
  location = nil, -- Current location from search results

  -- History cycling state
  history_offset = nil, -- Current offset in history (nil = not cycling, 0 = first query)
  next_search_force_combo_boost = false, -- Force combo boost on next search (for history recall)

  -- Combo state
  combo_visible = true, -- Whether to show combo indicator (hidden after significant navigation)
  combo_initial_cursor = nil, -- Initial cursor position when combo was shown

  -- Pagination state
  pagination = {
    page_index = 0, -- Current page index (0-based)
    page_size = 20, -- Items per page (updated dynamically)
    total_matched = 0, -- Total results from last search
    prefetch_margin = 5, -- Trigger refetch when within N items of edge
    -- Grep file-based pagination: stores the file_offset for each visited page
    -- so we can go backwards. grep_file_offsets[1] = 0 (page 0 starts at file 0),
    -- grep_file_offsets[2] = next_file_offset from page 0, etc.
    grep_file_offsets = {},
    grep_next_file_offset = 0,
  },

  config = nil,

  -- Custom renderer (optional, defaults to file_renderer if not provided)
  renderer = nil,

  ns_id = nil,

  last_status_info = nil,

  last_preview_file = nil,
  last_preview_location = nil, -- Track last preview location to detect changes

  preview_timer = nil, -- Separate timer for preview updates
  preview_debounce_ms = 100, -- Preview is more expensive, debounce more

  -- Set of selected file paths: { [filepath] = true }
  -- Uses Set pattern: selected items exist as keys with value true, deselected items are removed (nil)
  -- This allows O(1) lookup and automatic deduplication without needing to filter false values
  selected_files = {},

  -- Grep mode: per-occurrence selection keyed by "path:line:col"
  -- Values are the full item tables so quickfix can be built from selections alone (survives page changes)
  selected_items = {},

  -- Mode: nil or 'grep' — controls search/render/select behaviour
  mode = nil,
  -- Grep-specific config overrides (max_file_size, smart_case, etc.)
  grep_config = nil,
  -- Grep search mode: 'plain', 'regex', or 'fuzzy'
  grep_mode = 'plain',
  -- Regex fallback error: set when regex compilation fails and search fell back to literal
  grep_regex_fallback_error = nil,

  -- Cross-mode suggestion state: when primary search yields 0 results,
  -- we query the opposite mode and show those as suggestions.
  -- suggestion_items: array of items from the opposite search
  -- suggestion_source: 'grep' (suggestions from grep) or 'files' (suggestions from file search)
  suggestion_items = nil,
  suggestion_source = nil,
}

function M.create_ui()
  local config = M.state.config

  if not M.state.ns_id then
    M.state.ns_id = vim.api.nvim_create_namespace('fff_picker_status')
    combo_renderer.init(M.state.ns_id)
  end

  local debug_enabled_in_preview = M.enabled_preview() and config and config.debug and config.debug.show_file_info

  local terminal_width = vim.o.columns
  local terminal_height = vim.o.lines

  -- Calculate width and height (support function or number)
  local width_ratio = utils.resolve_config_value(
    config.layout.width,
    terminal_width,
    terminal_height,
    utils.is_valid_ratio,
    0.8,
    'layout.width'
  )
  local height_ratio = utils.resolve_config_value(
    config.layout.height,
    terminal_width,
    terminal_height,
    utils.is_valid_ratio,
    0.8,
    'layout.height'
  )

  local width = math.floor(terminal_width * width_ratio)
  local height = math.floor(terminal_height * height_ratio)

  -- Calculate col and row (support function or number)
  local col_ratio_default = 0.5 - (width_ratio / 2) -- default center
  local col_ratio
  if config.layout.col ~= nil then
    col_ratio = utils.resolve_config_value(
      config.layout.col,
      terminal_width,
      terminal_height,
      utils.is_valid_ratio,
      col_ratio_default,
      'layout.col'
    )
  else
    col_ratio = col_ratio_default
  end
  local row_ratio_default = 0.5 - (height_ratio / 2) -- default center
  local row_ratio
  if config.layout.row ~= nil then
    row_ratio = utils.resolve_config_value(
      config.layout.row,
      terminal_width,
      terminal_height,
      utils.is_valid_ratio,
      row_ratio_default,
      'layout.row'
    )
  else
    row_ratio = row_ratio_default
  end

  local col = math.floor(terminal_width * col_ratio)
  local row = math.floor(terminal_height * row_ratio)

  local prompt_position = get_prompt_position()
  local preview_position = get_preview_position()

  local preview_size_ratio = utils.resolve_config_value(
    config.layout.preview_size,
    terminal_width,
    terminal_height,
    utils.is_valid_ratio,
    0.4,
    'layout.preview_size'
  )

  local layout_config = {
    total_width = width,
    total_height = height,
    start_col = col,
    start_row = row,
    preview_position = preview_position,
    prompt_position = prompt_position,
    debug_enabled = debug_enabled_in_preview,
    preview_width = M.enabled_preview() and math.floor(width * preview_size_ratio) or 0,
    preview_height = M.enabled_preview() and math.floor(height * preview_size_ratio) or 0,
    separator_width = 3,
    file_info_height = debug_enabled_in_preview and 10 or 0,
  }

  local layout = M.calculate_layout_dimensions(layout_config)
  M.state.layout = layout

  M.state.input_buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_option(M.state.input_buf, 'bufhidden', 'wipe')

  M.state.list_buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_option(M.state.list_buf, 'bufhidden', 'wipe')

  if M.enabled_preview() then
    M.state.preview_buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_option(M.state.preview_buf, 'bufhidden', 'wipe')
  end

  if debug_enabled_in_preview then
    M.state.file_info_buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_option(M.state.file_info_buf, 'bufhidden', 'wipe')
  else
    M.state.file_info_buf = nil
  end

  local border_chars, t_junctions = get_border_chars()
  local list_window_config = {
    relative = 'editor',
    width = layout.list_width,
    height = layout.list_height,
    col = layout.list_col,
    row = layout.list_row,
    -- To make the input feel connected with the picker, we customize the
    -- respective corner border characters based on prompt_position
    -- When prompt at bottom: list has top border + sides, no bottom (connects to input below)
    -- When prompt at top: list has sides + bottom with T-junctions at top (connects to input above)
    border = prompt_position == 'bottom'
        and { border_chars[1], border_chars[2], border_chars[3], border_chars[4], '', '', '', border_chars[8] }
      or {
        t_junctions[1],
        border_chars[2],
        t_junctions[2],
        border_chars[4],
        border_chars[5],
        border_chars[6],
        border_chars[7],
        border_chars[8],
      },
    style = 'minimal',
  }

  local title = ' ' .. (M.state.config.title or 'FFFiles') .. ' '
  -- Only add title if prompt is at bottom - when prompt is top, title should be on input
  if prompt_position == 'bottom' then
    list_window_config.title = title
    list_window_config.title_pos = 'left'
  end

  M.state.list_win = vim.api.nvim_open_win(M.state.list_buf, false, list_window_config)

  -- Create file info window if debug enabled
  if debug_enabled_in_preview and layout.file_info then
    M.state.file_info_win = vim.api.nvim_open_win(M.state.file_info_buf, false, {
      relative = 'editor',
      width = layout.file_info.width,
      height = layout.file_info.height,
      col = layout.file_info.col,
      row = layout.file_info.row,
      style = 'minimal',
      border = border_chars,
      title = ' File Info ',
      title_pos = 'left',
    })
  else
    M.state.file_info_win = nil
  end

  -- Create preview window
  if M.enabled_preview() and layout.preview then
    M.state.preview_win = vim.api.nvim_open_win(M.state.preview_buf, false, {
      relative = 'editor',
      width = layout.preview.width,
      height = layout.preview.height,
      col = layout.preview.col,
      row = layout.preview.row,
      style = 'minimal',
      border = border_chars,
      title = ' Preview ',
      title_pos = 'left',
    })
  end

  local input_window_config = {
    relative = 'editor',
    width = layout.input_width,
    height = 1,
    col = layout.input_col,
    row = layout.input_row,
    -- To make the input feel connected with the picker, we customize the
    -- respective corner border characters based on prompt_position
    -- if prompt at bottom: input has T-junctions at top (connects to list above), full bottom border
    -- if prompt at top: input has top border + sides, no bottom (connects to list below)
    border = prompt_position == 'bottom' and {
      t_junctions[1],
      border_chars[2],
      t_junctions[2],
      border_chars[4],
      border_chars[5],
      border_chars[6],
      border_chars[7],
      border_chars[8],
    } or { border_chars[1], border_chars[2], border_chars[3], border_chars[4], '', '', '', border_chars[8] },
    style = 'minimal',
  }

  if prompt_position == 'top' then
    input_window_config.title = title
    input_window_config.title_pos = 'left'
  end

  M.state.input_win = vim.api.nvim_open_win(M.state.input_buf, false, input_window_config)

  M.setup_buffers()
  M.setup_windows()
  M.setup_keymaps()

  vim.api.nvim_set_current_win(M.state.input_win)

  preview.set_preview_window(M.state.preview_win)

  M.update_results_sync()
  M.clear_preview()
  M.update_status()

  return true
end

function M.setup_buffers()
  vim.api.nvim_buf_set_name(M.state.input_buf, 'fffile search')
  vim.api.nvim_buf_set_name(M.state.list_buf, 'fffiles list')
  if M.enabled_preview() then vim.api.nvim_buf_set_name(M.state.preview_buf, 'fffile preview') end

  vim.api.nvim_buf_set_option(M.state.input_buf, 'buftype', 'prompt')
  vim.api.nvim_buf_set_option(M.state.input_buf, 'filetype', 'fff_input')

  vim.fn.prompt_setprompt(M.state.input_buf, M.state.config.prompt)

  -- Changing the contents of the input buffer will trigger Neovim to guess the language in order to provide
  -- syntax highlighting. This makes sure that it's always off.
  vim.api.nvim_create_autocmd('Syntax', {
    buffer = M.state.input_buf,
    callback = function() vim.api.nvim_buf_set_option(M.state.input_buf, 'syntax', '') end,
  })

  vim.api.nvim_buf_set_option(M.state.list_buf, 'buftype', 'nofile')
  vim.api.nvim_buf_set_option(M.state.list_buf, 'filetype', 'fff_list')
  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', false)

  if M.state.file_info_buf then
    vim.api.nvim_buf_set_option(M.state.file_info_buf, 'buftype', 'nofile')
    vim.api.nvim_buf_set_option(M.state.file_info_buf, 'filetype', 'fff_file_info')
    vim.api.nvim_buf_set_option(M.state.file_info_buf, 'modifiable', false)
  end

  if M.enabled_preview() then
    vim.api.nvim_buf_set_option(M.state.preview_buf, 'buftype', 'nofile')
    vim.api.nvim_buf_set_option(M.state.preview_buf, 'filetype', 'fff_preview')
    vim.api.nvim_buf_set_option(M.state.preview_buf, 'modifiable', false)
  end
end

function M.setup_windows()
  local hl = M.state.config.hl
  local win_hl = string.format('Normal:%s,FloatBorder:%s,FloatTitle:%s', hl.normal, hl.border, hl.title)

  vim.api.nvim_win_set_option(M.state.input_win, 'wrap', false)
  vim.api.nvim_win_set_option(M.state.input_win, 'cursorline', false)
  vim.api.nvim_win_set_option(M.state.input_win, 'number', false)
  vim.api.nvim_win_set_option(M.state.input_win, 'relativenumber', false)
  vim.api.nvim_win_set_option(M.state.input_win, 'signcolumn', 'no')
  vim.api.nvim_win_set_option(M.state.input_win, 'foldcolumn', '0')
  vim.api.nvim_win_set_option(M.state.input_win, 'winhighlight', win_hl)

  vim.api.nvim_win_set_option(M.state.list_win, 'wrap', false)
  vim.api.nvim_win_set_option(M.state.list_win, 'cursorline', false)
  vim.api.nvim_win_set_option(M.state.list_win, 'number', false)
  vim.api.nvim_win_set_option(M.state.list_win, 'relativenumber', false)
  vim.api.nvim_win_set_option(M.state.list_win, 'signcolumn', 'yes:1') -- Enable signcolumn for git status borders
  vim.api.nvim_win_set_option(M.state.list_win, 'foldcolumn', '0')
  vim.api.nvim_win_set_option(M.state.list_win, 'winhighlight', win_hl)

  if M.enabled_preview() then
    vim.api.nvim_win_set_option(M.state.preview_win, 'wrap', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'cursorline', M.state.mode == 'grep')
    vim.api.nvim_win_set_option(
      M.state.preview_win,
      'number',
      M.state.mode == 'grep' or (preview_config and preview_config.line_numbers or false)
    )
    vim.api.nvim_win_set_option(M.state.preview_win, 'relativenumber', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'signcolumn', 'no')
    vim.api.nvim_win_set_option(M.state.preview_win, 'foldcolumn', '0')
    vim.api.nvim_win_set_option(M.state.preview_win, 'winhighlight', win_hl)
  end

  local picker_group = vim.api.nvim_create_augroup('fff_picker_focus', { clear = true })

  --- Check if a window is one of the picker windows
  --- @param win number Window handle to check
  --- @return boolean
  local function is_picker_window(win)
    if not win or not vim.api.nvim_win_is_valid(win) then return false end

    local picker_windows = { M.state.input_win, M.state.list_win }
    if M.state.preview_win then table.insert(picker_windows, M.state.preview_win) end
    if M.state.file_info_win then table.insert(picker_windows, M.state.file_info_win) end

    for _, picker_win in ipairs(picker_windows) do
      if picker_win and vim.api.nvim_win_is_valid(picker_win) and win == picker_win then return true end
    end

    return false
  end

  vim.api.nvim_create_autocmd('WinLeave', {
    group = picker_group,
    callback = function()
      if not M.state.active then return end

      local leaving_win = vim.api.nvim_get_current_win()

      -- Only care if we're leaving a picker window
      if not is_picker_window(leaving_win) then return end

      -- Schedule check to allow the window switch to complete
      vim.schedule(function()
        if not M.state.active then return end

        local new_win = vim.api.nvim_get_current_win()

        -- Close picker only if we moved to a non-picker window
        if not is_picker_window(new_win) then M.close() end
      end)
    end,
    desc = 'Close picker when focus leaves picker windows',
  })
end

local function set_keymap(mode, keys, handler, opts)
  local normalized_keys

  if type(keys) == 'string' then
    normalized_keys = { keys }
  elseif type(keys) == 'table' then
    normalized_keys = keys
  else
    normalized_keys = {}
  end

  for _, key in ipairs(normalized_keys) do
    vim.keymap.set(mode, key, handler, opts)
  end
end

function M.focus_list_win()
  if not M.state.active then return end
  if not M.state.list_win or not vim.api.nvim_win_is_valid(M.state.list_win) then return end

  vim.cmd('stopinsert')
  vim.api.nvim_set_current_win(M.state.list_win)
end

function M.focus_preview_win()
  if not M.state.active then return end
  if not M.state.preview_win or not vim.api.nvim_win_is_valid(M.state.preview_win) then return end

  vim.cmd('stopinsert')
  vim.api.nvim_set_current_win(M.state.preview_win)
end

local function move_list_cursor(direction)
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 then return end

  local new_cursor = M.state.cursor + direction
  new_cursor = math.max(1, math.min(new_cursor, #items))

  if new_cursor ~= M.state.cursor then
    M.state.cursor = new_cursor
    M.render_list()
    if M.state.mode == 'grep' or M.state.suggestion_source == 'grep' then
      M.update_preview_smart()
    else
      M.update_preview()
    end
    M.update_status()
  end
end

function M.setup_keymaps()
  local keymaps = M.state.config.keymaps
  local input_opts = { buffer = M.state.input_buf, noremap = true, silent = true }
  local list_opts = { buffer = M.state.list_buf, noremap = true, silent = true }

  vim.keymap.set('i', '<C-w>', function()
    local col = vim.fn.col('.') - 1
    local line = vim.fn.getline('.')
    local prompt_len = #M.state.config.prompt
    if col <= prompt_len then return '' end
    local text_part = line:sub(prompt_len + 1, col)
    local after_cursor = line:sub(col + 1)
    local new_text = text_part:gsub('%S*%s*$', '')
    local new_line = M.state.config.prompt .. new_text .. after_cursor
    local new_col = prompt_len + #new_text
    vim.fn.setline('.', new_line)
    vim.fn.cursor(vim.fn.line('.'), new_col + 1)
    return ''
  end, input_opts)

  set_keymap('i', keymaps.move_up, M.move_up, input_opts)
  set_keymap('i', keymaps.move_down, M.move_down, input_opts)
  set_keymap('i', keymaps.cycle_previous_query, M.recall_query_from_history, input_opts)
  set_keymap('n', 'j', M.move_down, input_opts)
  set_keymap('n', 'k', M.move_up, input_opts)
  set_keymap('n', keymaps.focus_list, M.focus_list_win, input_opts)
  set_keymap('n', keymaps.focus_preview, M.focus_preview_win, input_opts)

  -- Input buffer: both modes
  set_keymap({ 'i', 'n' }, keymaps.close, M.close, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.select, M.select, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.select_split, function() M.select('split') end, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.select_vsplit, function() M.select('vsplit') end, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.select_tab, function() M.select('tab') end, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.preview_scroll_up, M.scroll_preview_up, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.preview_scroll_down, M.scroll_preview_down, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.toggle_debug, M.toggle_debug, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.toggle_select, M.toggle_select, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.send_to_quickfix, M.send_to_quickfix, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.toggle_grep_regex, M.toggle_grep_regex, input_opts)

  -- List buffer
  set_keymap('n', keymaps.close, M.close, list_opts)
  set_keymap('n', 'q', M.close, list_opts)
  set_keymap('n', 'j', function() move_list_cursor(1) end, list_opts)
  set_keymap('n', 'k', function() move_list_cursor(-1) end, list_opts)
  set_keymap('n', 'i', M.focus_input_win, list_opts)
  set_keymap('n', keymaps.focus_preview, M.focus_preview_win, list_opts)
  set_keymap('n', keymaps.select, M.select, list_opts)
  set_keymap('n', keymaps.select_split, function() M.select('split') end, list_opts)
  set_keymap('n', keymaps.select_vsplit, function() M.select('vsplit') end, list_opts)
  set_keymap('n', keymaps.select_tab, function() M.select('tab') end, list_opts)
  set_keymap('n', keymaps.preview_scroll_up, M.scroll_preview_up, list_opts)
  set_keymap('n', keymaps.preview_scroll_down, M.scroll_preview_down, list_opts)
  set_keymap('n', keymaps.toggle_debug, M.toggle_debug, list_opts)
  set_keymap('n', keymaps.toggle_select, M.toggle_select, list_opts)
  set_keymap('n', keymaps.send_to_quickfix, M.send_to_quickfix, list_opts)

  -- Preview buffer
  if M.state.preview_buf then
    local preview_opts = { buffer = M.state.preview_buf, noremap = true, silent = true }

    set_keymap('n', keymaps.close, M.close, preview_opts)
    set_keymap('n', 'q', M.close, preview_opts)
    set_keymap('n', 'i', M.focus_input_win, preview_opts)
    set_keymap('n', keymaps.focus_list, M.focus_list_win, preview_opts)
    set_keymap('n', keymaps.select, M.select, preview_opts)
    set_keymap('n', keymaps.select_split, function() M.select('split') end, preview_opts)
    set_keymap('n', keymaps.select_vsplit, function() M.select('vsplit') end, preview_opts)
    set_keymap('n', keymaps.select_tab, function() M.select('tab') end, preview_opts)
    set_keymap('n', keymaps.toggle_debug, M.toggle_debug, preview_opts)
    set_keymap('n', keymaps.toggle_select, M.toggle_select, preview_opts)
    set_keymap('n', keymaps.send_to_quickfix, M.send_to_quickfix, preview_opts)
  end

  vim.api.nvim_buf_attach(M.state.input_buf, false, {
    on_lines = function()
      vim.schedule(function() M.on_input_change() end)
    end,
  })
end

function M.focus_input_win()
  if not M.state.active then return end
  if not M.state.input_win or not vim.api.nvim_win_is_valid(M.state.input_win) then return end

  vim.api.nvim_set_current_win(M.state.input_win)

  vim.api.nvim_win_call(M.state.input_win, function() vim.cmd('startinsert!') end)
end

function M.toggle_debug()
  local config_changed = conf.toggle_debug()
  if config_changed then
    local current_query = M.state.query
    local current_items = M.state.items
    local current_cursor = M.state.cursor
    -- Preserve mode-specific state across close/open cycle
    local current_mode = M.state.mode
    local current_renderer = M.state.renderer
    local current_grep_mode = M.state.grep_mode
    local current_grep_config = M.state.grep_config
    local current_filtered_items = M.state.filtered_items
    local current_selected_files = M.state.selected_files
    local current_selected_items = M.state.selected_items

    M.close()
    M.open({
      mode = current_mode,
      renderer = current_renderer,
      grep_config = current_grep_config,
    })

    M.state.query = current_query
    M.state.items = current_items
    M.state.cursor = current_cursor
    M.state.grep_mode = current_grep_mode
    M.state.filtered_items = current_filtered_items
    M.state.selected_files = current_selected_files
    M.state.selected_items = current_selected_items
    M.render_list()
    M.update_preview()
    M.update_status()

    vim.schedule(function()
      if M.state.active and M.state.input_win then
        vim.api.nvim_set_current_win(M.state.input_win)
        vim.cmd('startinsert!')
      end
    end)
  else
    M.update_results()
  end
end

--- Cycle through grep search modes based on configured modes list.
--- Only works when the picker is in grep mode. Triggers a re-search
--- with the current query using the new mode.
function M.toggle_grep_regex()
  if not M.state.active or M.state.mode ~= 'grep' then return end

  local config = conf.get()
  -- Use grep_config.modes if provided, otherwise fall back to global config
  local modes = (M.state.grep_config and M.state.grep_config.modes)
    or config.grep.modes
    or { 'plain', 'regex', 'fuzzy' }

  -- If only one mode configured, no cycling needed
  if #modes <= 1 then return end

  local current_idx = 1
  for i, m in ipairs(modes) do
    if m == M.state.grep_mode then
      current_idx = i
      break
    end
  end
  M.state.grep_mode = modes[(current_idx % #modes) + 1]

  -- Clear fallback error when switching away from regex
  if M.state.grep_mode ~= 'regex' then M.state.grep_regex_fallback_error = nil end

  -- Force status refresh by clearing the cached value
  M.state.last_status_info = nil
  M.update_status()

  -- Re-run the search with the current query in the new mode
  if M.state.query ~= '' then M.update_results_sync() end
end

function M.on_input_change()
  if not M.state.active then return end

  local lines = vim.api.nvim_buf_get_lines(M.state.input_buf, 0, -1, false)
  local prompt_len = #M.state.config.prompt
  local query = ''

  if #lines > 1 then
    -- join without any separator because it is a use case for a path copy from the terminal buffer
    local all_text = table.concat(lines, '')
    if all_text:sub(1, prompt_len) == M.state.config.prompt then
      query = all_text:sub(prompt_len + 1)
    else
      query = all_text
    end

    query = query:gsub('\r', ''):match('^%s*(.-)%s*$') or ''

    vim.api.nvim_buf_set_option(M.state.input_buf, 'modifiable', true)
    vim.api.nvim_buf_set_lines(M.state.input_buf, 0, -1, false, { M.state.config.prompt .. query })

    -- Move cursor to end
    vim.schedule(function()
      if M.state.active and M.state.input_win and vim.api.nvim_win_is_valid(M.state.input_win) then
        vim.api.nvim_win_set_cursor(M.state.input_win, { 1, prompt_len + #query })
      end
    end)
  else
    local full_line = lines[1] or ''
    if full_line:sub(1, prompt_len) == M.state.config.prompt then query = full_line:sub(prompt_len + 1) end
  end

  M.state.query = query

  M.update_results_sync()
end

function M.update_results() M.update_results_sync() end

function M.update_results_sync()
  if not M.state.active then return end

  if not M.state.current_file_cache then
    local current_buf = vim.api.nvim_get_current_buf()
    if current_buf and vim.api.nvim_buf_is_valid(current_buf) then
      local current_file = vim.api.nvim_buf_get_name(current_buf)
      M.state.current_file_cache = (current_file ~= '' and vim.fn.filereadable(current_file) == 1) and current_file
        or nil
    end
  end

  local prompt_position = get_prompt_position()

  -- Calculate page size dynamically based on window height
  local page_size
  if M.state.list_win and vim.api.nvim_win_is_valid(M.state.list_win) then
    page_size = vim.api.nvim_win_get_height(M.state.list_win)
  else
    page_size = M.state.config.max_results or 100
  end

  -- Update pagination state
  M.state.pagination.page_size = page_size
  M.state.pagination.page_index = 0 -- Reset to first page on new search

  -- Reset combo visibility on new search
  M.state.combo_visible = true
  M.state.combo_initial_cursor = 1 -- Will be at position 1 after search

  -- Check if we should force combo boost for this search (history recall)
  local min_combo_override = nil
  if M.state.next_search_force_combo_boost then
    min_combo_override = 0 -- Force combo boost by setting min_combo_count to 0
  end

  local results
  if M.state.mode == 'grep' then
    M.state.grep_regex_fallback_error = nil
    if M.state.query == '' then
      -- Empty query: show empty state (no search needed)
      results = {}
      M.state.pagination.total_matched = 0
      M.state.pagination.grep_file_offsets = {}
      M.state.pagination.grep_next_file_offset = 0
    else
      -- Grep mode: use live_grep search (file_offset=0 for first page)
      local grep = require('fff.grep')
      local grep_result = grep.search(M.state.query, 0, page_size, M.state.grep_config, M.state.grep_mode)
      results = grep_result.items or {}
      M.state.pagination.total_matched = grep_result.total_matched or 0
      M.state.pagination.grep_file_offsets = { 0 } -- Page 0 starts at file 0
      M.state.pagination.grep_next_file_offset = grep_result.next_file_offset or 0
      M.state.grep_regex_fallback_error = grep_result.regex_fallback_error or nil
      -- Record offset for page 1 so forward navigation works immediately
      if grep_result.next_file_offset and grep_result.next_file_offset > 0 then
        M.state.pagination.grep_file_offsets[2] = grep_result.next_file_offset
      end
    end
    M.state.location = nil -- Location comes from selected item, not query
  else
    -- File picker mode: use fuzzy search
    results = file_picker.search_files_paginated(
      M.state.query,
      M.state.current_file_cache,
      M.state.config.max_threads,
      min_combo_override,
      0,
      page_size
    )

    -- Get location from search results
    M.state.location = file_picker.get_search_location()

    local metadata = file_picker.get_search_metadata()
    M.state.pagination.total_matched = metadata.total_matched
  end

  M.state.items = results
  M.state.filtered_items = results

  -- Cross-mode suggestions: when primary search yields 0 results with a non-empty query,
  -- query the opposite mode and store results as suggestions.
  M.state.suggestion_items = nil
  M.state.suggestion_source = nil
  if #results == 0 and M.state.query ~= '' then
    if M.state.mode == 'grep' then
      -- Grep returned nothing — try file search as suggestion
      local suggestion_results = file_picker.search_files_paginated(
        M.state.query,
        M.state.current_file_cache,
        M.state.config.max_threads,
        nil,
        0,
        page_size
      )
      if suggestion_results and #suggestion_results > 0 then
        M.state.suggestion_items = suggestion_results
        M.state.suggestion_source = 'files'
      end
    else
      -- File search returned nothing — try grep as suggestion
      local grep = require('fff.grep')
      local grep_result = grep.search(M.state.query, 0, page_size, M.state.grep_config, false)
      local grep_items = grep_result and grep_result.items or {}
      if #grep_items > 0 then
        M.state.suggestion_items = grep_items
        M.state.suggestion_source = 'grep'
      end
    end
  end

  -- When suggestions are available, use them as the navigable item list
  -- so the user can browse and select them with normal keybindings.
  if M.state.suggestion_items and #M.state.suggestion_items > 0 then
    M.state.filtered_items = M.state.suggestion_items
  end

  -- Results always come in descending order (best first) from Rust
  -- For bottom prompt, we render in reverse so best items appear at bottom
  -- But cursor index should still point to items[1] (best item)
  M.state.cursor = #M.state.filtered_items > 0 and 1 or 1

  M.render_debounced()
end

--- Load page with given page index
function M.load_page_at_index(new_page_index, adjust_cursor_fn)
  local page_size = M.state.pagination.page_size

  -- Protect against division by zero
  if page_size == 0 then return false end

  if M.state.mode ~= 'grep' then
    local total = M.state.pagination.total_matched
    if total == 0 then return false end

    -- Calculate max page index
    local max_page_index = math.max(0, math.ceil(total / page_size) - 1)

    -- Clamp page_index to valid range
    new_page_index = math.max(0, math.min(new_page_index, max_page_index))
  end

  local prompt_position = get_prompt_position()

  local ok, results
  if M.state.mode == 'grep' then
    -- File-based pagination: look up the file_offset for this page from our history
    local file_offset = M.state.pagination.grep_file_offsets[new_page_index + 1] -- 1-based Lua index
    if file_offset == nil then
      -- We don't have a recorded offset for this page (shouldn't happen in normal flow)
      return false
    end

    local grep = require('fff.grep')
    ok, results = pcall(grep.search, M.state.query, file_offset, page_size, M.state.grep_config, M.state.grep_mode)
    if ok and results then
      local grep_result = results
      results = grep_result.items or {}
      M.state.pagination.total_matched = grep_result.total_matched or 0
      M.state.pagination.grep_next_file_offset = grep_result.next_file_offset or 0
      M.state.grep_regex_fallback_error = grep_result.regex_fallback_error or nil

      -- Record the offset for the NEXT page so forward navigation works
      if grep_result.next_file_offset and grep_result.next_file_offset > 0 then
        M.state.pagination.grep_file_offsets[new_page_index + 2] = grep_result.next_file_offset
      end
    end
  else
    ok, results = pcall(
      file_picker.search_files_paginated,
      M.state.query,
      M.state.current_file_cache,
      M.state.config.max_threads,
      nil, -- No combo boost override for page navigation
      new_page_index,
      page_size
    )
  end

  if not ok then
    vim.notify('Error in paginated search: ' .. tostring(results), vim.log.levels.ERROR)
    vim.api.nvim_err_writeln('FFF ERROR: Paginated search failed: ' .. tostring(results))
    return false
  end

  if #results == 0 then return false end

  -- CRITICAL: Update total_matched from the latest search metadata
  -- This prevents stale total_matched values that can cause out-of-bounds pagination
  -- For grep, total_matched was already updated above when extracting grep_result.
  if M.state.mode ~= 'grep' then
    local metadata = file_picker.get_search_metadata()
    M.state.pagination.total_matched = metadata.total_matched
  end

  M.state.items = results
  M.state.filtered_items = results
  M.state.pagination.page_index = new_page_index

  -- Adjust cursor position (provided by caller)
  if adjust_cursor_fn then
    local ok, err = pcall(adjust_cursor_fn, #results)
    if not ok then
      vim.notify('Error in cursor adjustment: ' .. tostring(err), vim.log.levels.ERROR)
      return false
    end
  end

  local ok, err = pcall(M.render_list)
  if not ok then
    vim.notify('Error in render_list: ' .. tostring(err), vim.log.levels.ERROR)
    return false
  end

  ok, err = pcall(M.update_preview)
  if not ok then
    vim.notify('Error in update_preview: ' .. tostring(err), vim.log.levels.ERROR)
    return false
  end

  ok, err = pcall(M.update_status)
  if not ok then
    vim.notify('Error in update_status: ' .. tostring(err), vim.log.levels.ERROR)
    return false
  end
  return true
end

--- Load next page (scroll down reached end)
function M.load_next_page()
  local page_size = M.state.pagination.page_size
  local current_page = M.state.pagination.page_index

  -- Protect against division by zero
  if page_size == 0 then return false end

  if M.state.mode == 'grep' then
    -- File-based pagination: check if there are more files to search
    if M.state.pagination.grep_next_file_offset == 0 then
      return false -- No more files
    end
    local new_page_index = current_page + 1
    return M.load_page_at_index(new_page_index, function(result_count) M.state.cursor = 1 end)
  end

  local total = M.state.pagination.total_matched
  if total == 0 then return false end

  local max_page_index = math.max(0, math.ceil(total / page_size) - 1)
  if current_page >= max_page_index then return false end

  local new_page_index = current_page + 1

  return M.load_page_at_index(new_page_index, function(result_count) M.state.cursor = 1 end)
end

--- Load previous page (scroll up reached beginning)
function M.load_previous_page()
  if M.state.pagination.page_index == 0 then return false end

  local new_page_index = M.state.pagination.page_index - 1
  local prompt_position = get_prompt_position()

  return M.load_page_at_index(new_page_index, function(result_count) M.state.cursor = result_count end)
end

function M.update_preview_debounced()
  -- Cancel previous preview timer
  if M.state.preview_timer then
    M.state.preview_timer:stop()
    M.state.preview_timer:close()
    M.state.preview_timer = nil
  end

  -- Create new timer with longer debounce for expensive preview
  M.state.preview_timer = vim.loop.new_timer()
  M.state.preview_timer:start(
    M.state.preview_debounce_ms,
    0,
    vim.schedule_wrap(function()
      if M.state.active then
        M.update_preview()
        M.state.preview_timer = nil
      end
    end)
  )
end

--- Smart preview update for cursor movement.
--- Same-file location changes are instant; file changes are debounced
--- to avoid visible preview flicker when scrolling rapidly through grep results.
function M.update_preview_smart()
  if not M.enabled_preview() then return end
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then
    M.update_preview()
    return
  end

  local item = items[M.state.cursor]
  if not item then
    M.update_preview()
    return
  end

  -- Same file: update immediately (just scrolling/re-highlighting, no file I/O)
  if M.state.last_preview_file == item.path then
    M.update_preview()
    return
  end

  -- Different file: debounce to avoid flicker during rapid scrolling
  M.update_preview_debounced()
end

function M.render_debounced()
  vim.schedule(function()
    if M.state.active then
      M.render_list()
      M.update_preview()
      M.update_status()
    end
  end)
end

local function shrink_path(path, max_width)
  local config = conf.get()
  local strategy = config.layout and config.layout.path_shorten_strategy or 'middle_number'
  return rust.shorten_path(path, max_width, strategy)
end

local function format_file_display(item, max_width)
  local filename = item.name
  local dir_path = item.directory or ''

  if dir_path == '' and item.relative_path then
    local parent_dir = vim.fn.fnamemodify(item.relative_path, ':h')
    if parent_dir ~= '.' and parent_dir ~= '' then dir_path = parent_dir end
  end

  local filename_width = vim.fn.strdisplaywidth(filename)
  local base_width = filename_width + 1 -- filename + " "
  local path_max_width = math.max(max_width - base_width, 0)

  if dir_path == '' or path_max_width == 0 then return filename, '' end
  local display_path = shrink_path(dir_path, path_max_width)

  return filename, display_path
end

--- Adjust scroll for bottom prompt to eliminate gaps
local function scroll_to_bottom()
  if not M.state.list_win or not vim.api.nvim_win_is_valid(M.state.list_win) then return end

  local win_height = vim.api.nvim_win_get_height(M.state.list_win)
  local buf_lines = vim.api.nvim_buf_line_count(M.state.list_buf)

  vim.api.nvim_win_call(M.state.list_win, function()
    local view = vim.fn.winsaveview()
    -- Force topline to show content at bottom
    view.topline = math.max(1, buf_lines - win_height + 1)
    vim.fn.winrestview(view)
  end)
end

--- Render the grep empty state: tips + bordered section of recent files.
--- Called when grep mode has an empty query and no items.
local function render_grep_empty_state(ctx)
  local config = ctx.config
  local win_width = ctx.win_width
  local win_height = ctx.win_height
  local prompt_position = ctx.prompt_position

  local content = {}
  local hl_cmds = {} -- { row (0-based), col_start, col_end, hl_group }

  table.insert(content, '')
  table.insert(content, '  Start typing to search file contents...')
  table.insert(content, '')
  table.insert(content, '  Tips:')
  table.insert(content, '    "pattern *.rs"    search only in Rust files')
  table.insert(content, '    "pattern /src/"   limit search to src/ directory')
  table.insert(content, '    "!test pattern"   exclude test files')
  table.insert(content, '')

  table.insert(content, border_bot)

  -- For bottom prompt: push content to the bottom by prepending empty lines
  if prompt_position == 'bottom' then
    local empty_needed = math.max(0, win_height - #content)
    for _ = 1, empty_needed do
      table.insert(content, 1, string.rep(' ', win_width + 5))
      -- Shift all highlight rows
      for _, h in ipairs(hl_cmds) do
        h.row = h.row + 1
      end
    end
  end

  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', true)
  vim.api.nvim_buf_set_lines(M.state.list_buf, 0, -1, false, content)
  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', false)

  vim.api.nvim_buf_clear_namespace(M.state.list_buf, M.state.ns_id, 0, -1)

  -- For bottom prompt, ensure empty state is anchored at the bottom
  if prompt_position == 'bottom' then scroll_to_bottom() end

  -- Apply highlights
  for _, h in ipairs(hl_cmds) do
    pcall(vim.api.nvim_buf_add_highlight, M.state.list_buf, M.state.ns_id, h.hl, h.row, h.col_start, h.col_end)
  end

  local tip_offset = prompt_position == 'bottom' and math.max(0, win_height - #content + (win_height - #content)) or 0
  for i = 0, #content - 1 do
    local line = content[i + 1]
    if
      line and (line:match('^%s+Start typing') or line:match('^%s+Tips') or line:match('^%s+"') or line:match('^%s+!'))
    then
      pcall(vim.api.nvim_buf_add_highlight, M.state.list_buf, M.state.ns_id, 'Comment', i, 0, -1)
    end
    -- Dim border characters
    if line and (line:match('^%s+[╭╰│]') or line:match('[╮╯│]%s*$')) then
      pcall(
        vim.api.nvim_buf_add_highlight,
        M.state.list_buf,
        M.state.ns_id,
        config.hl.border or 'FloatBorder',
        i,
        0,
        -1
      )
    end
  end
end

--- Get the appropriate renderer for cross-mode suggestions.
--- When file search yields no results we suggest grep results (use grep_renderer),
--- and vice versa (use file_renderer).
---@return table renderer
function M.get_suggestion_renderer()
  if M.state.suggestion_source == 'grep' then
    return require('fff.grep.grep_renderer')
  else
    return require('fff.file_renderer')
  end
end

--- Build rendering context with all necessary data
--- @return table Context object with items, config, dimensions, combo info, etc.
local function build_render_context()
  local config = conf.get()
  local items = M.state.filtered_items
  local win_height = vim.api.nvim_win_get_height(M.state.list_win)
  local win_width = vim.api.nvim_win_get_width(M.state.list_win)
  local prompt_position = get_prompt_position()

  -- Get actual text offset (signcolumn + foldcolumn + line numbers)
  local win_info = vim.fn.getwininfo(M.state.list_win)[1]
  local text_offset = win_info and win_info.textoff or 2
  local text_width = win_width - text_offset

  -- Cursor validation
  if M.state.cursor < 1 then
    M.state.cursor = 1
  elseif M.state.cursor > #items then
    M.state.cursor = #items
  end

  -- Combo detection (only in file picker mode with real results, not grep or suggestions)
  local combo_boost_score_multiplier = config.history and config.history.combo_boost_multiplier or 100
  local has_combo, combo_header_line, combo_header_text_len, combo_item_index
  if M.state.mode == 'grep' or M.state.suggestion_source then
    has_combo = false
    combo_header_line = nil
    combo_header_text_len = nil
    combo_item_index = nil
  else
    has_combo, combo_header_line, combo_header_text_len, combo_item_index = combo_renderer.detect_and_prepare(
      items,
      file_picker,
      win_width,
      combo_boost_score_multiplier,
      M.state.next_search_force_combo_boost or config.history.min_combo_count == 0
    )
  end
  M.state.next_search_force_combo_boost = false

  if has_combo and not M.state.combo_visible then
    has_combo = false
    combo_item_index = nil
  end

  -- Determine iteration order
  local display_start = 1
  local display_end = #items
  local iter_start, iter_end, iter_step
  if prompt_position == 'bottom' then
    iter_start, iter_end, iter_step = display_end, display_start, -1
  else
    iter_start, iter_end, iter_step = display_start, display_end, 1
  end

  return {
    config = config,
    items = items,
    cursor = M.state.cursor,
    win_height = win_height,
    win_width = win_width,
    max_path_width = text_width, -- Actual text area width (excluding signcolumn)
    debug_enabled = config and config.debug and config.debug.show_scores,
    prompt_position = prompt_position,
    has_combo = has_combo,
    combo_header_line = combo_header_line,
    combo_header_text_len = combo_header_text_len,
    combo_item_index = combo_item_index,
    display_start = display_start,
    display_end = display_end,
    iter_start = iter_start,
    iter_end = iter_end,
    iter_step = iter_step,
    renderer = M.state.suggestion_source and M.get_suggestion_renderer() or M.state.renderer,
    query = M.state.query, -- Current search query (used by grep renderer for empty-state detection)
    selected_files = M.state.selected_files, -- Selected files set (used by file renderer for selection markers)
    selected_items = M.state.selected_items, -- Selected items map (used by grep renderer for per-occurrence markers)
    mode = M.state.mode, -- Current mode (nil or 'grep')
    format_file_display = format_file_display, -- Helper for renderers to format filename + dir path
    suggestion_source = M.state.suggestion_source, -- Cross-mode suggestion source ('grep' or 'files')
  }
end

-- NOTE: Line generation, bottom padding, buffer writes, cursor positioning,
-- and highlight application are handled by list_renderer.lua.
-- picker_ui delegates to list_renderer.render() in render_list() below.

local function finalize_render(item_to_lines, ctx)
  local combo_text_len = nil
  if ctx.combo_item_index and item_to_lines[ctx.combo_item_index] then
    combo_text_len = item_to_lines[ctx.combo_item_index].combo_header_text_len
  end

  local combo_was_hidden = combo_renderer.render_highlights_and_overlays(
    ctx.combo_item_index,
    combo_text_len or ctx.combo_header_text_len,
    M.state.list_buf,
    M.state.list_win,
    M.state.ns_id,
    ctx.config.hl.border,
    item_to_lines,
    ctx.prompt_position,
    #ctx.items
  )

  -- it's important part of functionality when user scrolls to the middle of the page we hide
  -- the combo overlay which leaves the gap of the internal neovim buffer, so scroll to show last item
  if combo_was_hidden and ctx.prompt_position == 'bottom' then scroll_to_bottom() end

  -- Scrollbar is only meaningful for file picker mode where total_matched is exact.
  -- Grep uses early termination so total_matched is approximate — scrollbar would be misleading.
  -- Also skip scrollbar when showing cross-mode suggestions (total_matched reflects the primary search, not suggestions).
  if ctx.mode ~= 'grep' and not ctx.suggestion_source then
    scrollbar.render(M.state.layout, ctx.config, M.state.list_win, M.state.pagination, ctx.prompt_position)
  end
end

function M.render_list()
  if not M.state.active then return end

  local ctx = build_render_context()

  -- Grep empty state: render a welcome view instead of the normal item list
  if M.state.mode == 'grep' and #ctx.items == 0 then
    render_grep_empty_state(ctx)
    return
  end

  -- Delegate line generation, padding, buffer write, cursor, and highlights
  -- to the list_renderer module. It returns the item_to_lines mapping needed
  -- by finalize_render for combo overlays and scrollbar.
  local item_to_lines = list_renderer.render(ctx, M.state.list_buf, M.state.list_win, M.state.ns_id)

  -- For bottom prompt, always ensure content is anchored at the bottom after rendering
  -- This prevents results from appearing in the middle when there are few items
  if ctx.prompt_position == 'bottom' then scroll_to_bottom() end

  -- Finalize with combo overlays and scrollbar
  finalize_render(item_to_lines, ctx)
end

--- Build and set the preview window title for a given item and location.
--- For grep mode, appends :line to the path.
---@param item table The current item
---@param location table|nil The effective location
function M.update_preview_title(item, location)
  if not M.state.preview_win or not vim.api.nvim_win_is_valid(M.state.preview_win) then return end

  local relative_path = item.relative_path or item.path
  local max_title_width = vim.api.nvim_win_get_width(M.state.preview_win)

  -- Append :line for grep mode or grep suggestions
  local suffix = ''
  local is_grep_item = M.state.mode == 'grep' or M.state.suggestion_source == 'grep'
  if is_grep_item and location and location.line then suffix = ':' .. tostring(location.line) end

  local display_path = relative_path .. suffix
  local title

  if #display_path + 2 <= max_title_width then
    title = string.format(' %s ', display_path)
  else
    local available_chars = max_title_width - 2

    local filename = vim.fn.fnamemodify(relative_path, ':t') .. suffix
    if available_chars <= 3 then
      title = filename
    else
      if #filename + 5 <= available_chars then
        local normalized_path = vim.fs.normalize(relative_path)
        local path_parts = vim.split(normalized_path, '[/\\]', { plain = false })

        local segments = {}
        for _, part in ipairs(path_parts) do
          if part ~= '' then table.insert(segments, part) end
        end

        -- Replace last segment with filename+suffix
        segments[#segments] = vim.fn.fnamemodify(relative_path, ':t') .. suffix

        local segments_to_show = { segments[#segments] }
        local current_length = #segments_to_show[1] + 4 -- 4 for '../' prefix and spaces

        for i = #segments - 1, 1, -1 do
          local segment = segments[i]
          local new_length = current_length + #segment + 1 -- +1 for '/'

          if new_length <= available_chars then
            table.insert(segments_to_show, 1, segment)
            current_length = new_length
          else
            break
          end
        end

        if #segments_to_show == #segments then
          title = string.format(' %s ', table.concat(segments_to_show, '/'))
        else
          title = string.format(' ../%s ', table.concat(segments_to_show, '/'))
        end
      else
        local truncated = filename:sub(1, available_chars - 3) .. '...'
        title = string.format(' %s ', truncated)
      end
    end
  end

  vim.api.nvim_win_set_config(M.state.preview_win, {
    title = title,
    title_pos = 'left',
  })
end

function M.update_preview()
  if not M.enabled_preview() then return end
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then
    M.clear_preview()
    M.state.last_preview_file = nil
    M.state.last_preview_location = nil
    return
  end

  local item = items[M.state.cursor]
  if not item then
    M.clear_preview()
    M.state.last_preview_file = nil
    M.state.last_preview_location = nil
    return
  end

  -- Check if we need to update the preview (file changed OR location changed)
  local effective_location = M.state.location
  -- In grep mode (or when previewing grep suggestions), location comes from the match item
  local is_grep_item = M.state.mode == 'grep' or M.state.suggestion_source == 'grep'
  if is_grep_item and item.line_number and item.line_number > 0 then
    effective_location = { line = item.line_number }
    if item.col and item.col > 0 then
      effective_location.col = item.col + 1 -- Convert 0-based byte col to 1-based for highlight_location
    end
    -- Pass the query for multi-occurrence highlighting in preview (plain/regex modes).
    -- For fuzzy mode, also pass the per-match byte offsets so the preview can highlight
    -- the exact matched characters on the target line without re-searching.
    effective_location.grep_query = M.state.query
    if M.state.grep_mode == 'fuzzy' and item.match_ranges then
      effective_location.fuzzy_match_ranges = item.match_ranges
    end
  end

  local location_changed = not vim.deep_equal(M.state.last_preview_location, effective_location)

  if M.state.last_preview_file == item.path and not location_changed then return end

  -- Same file, different location: just scroll and re-highlight instead of reloading
  if M.state.last_preview_file == item.path and location_changed then
    M.state.last_preview_location = effective_location and vim.deepcopy(effective_location) or nil
    preview.state.location = effective_location
    -- Update title with new line number for grep/suggestion mode
    if is_grep_item and effective_location and effective_location.line then
      M.update_preview_title(item, effective_location)
    end
    if M.state.preview_buf and vim.api.nvim_buf_is_valid(M.state.preview_buf) then
      preview.apply_location_highlighting(M.state.preview_buf)
    end
    return
  end

  preview.clear()

  M.state.last_preview_file = item.path
  M.state.last_preview_location = effective_location and vim.deepcopy(effective_location) or nil

  M.update_preview_title(item, effective_location)

  if M.state.file_info_buf then preview.update_file_info_buffer(item, M.state.file_info_buf, M.state.cursor) end

  preview.set_preview_window(M.state.preview_win)
  preview.preview(item.path, M.state.preview_buf, effective_location, item.is_binary)
end

--- Clear preview
function M.clear_preview()
  if not M.state.active then return end
  if not M.enabled_preview() then return end

  vim.api.nvim_win_set_config(M.state.preview_win, {
    title = ' Preview ',
    title_pos = 'left',
  })

  if M.state.file_info_buf then
    vim.api.nvim_buf_set_option(M.state.file_info_buf, 'modifiable', true)
    vim.api.nvim_buf_set_lines(M.state.file_info_buf, 0, -1, false, {
      'File Info Panel',
      '',
      'Select a file to view:',
      '• Comprehensive scoring details',
      '• File size and type information',
      '• Git status integration',
      '• Modification & access timings',
      '• Frecency scoring breakdown',
      '',
      'Navigate: ↑↓ or Ctrl+p/n',
    })
    vim.api.nvim_buf_set_option(M.state.file_info_buf, 'modifiable', false)
  end

  vim.api.nvim_buf_set_option(M.state.preview_buf, 'modifiable', true)
  vim.api.nvim_buf_set_lines(M.state.preview_buf, 0, -1, false, { 'No preview available' })
  vim.api.nvim_buf_set_option(M.state.preview_buf, 'modifiable', false)
end

--- Update status information on the right side of input using virtual text
function M.update_status(progress)
  if not M.state.active or not M.state.ns_id then return end
  local config = M.state.config

  if M.state.mode == 'grep' then
    -- Determine available modes to decide if we should show the mode indicator
    -- Use grep_config.modes if provided, otherwise fall back to global config
    local modes = (M.state.grep_config and M.state.grep_config.modes)
      or config.grep.modes
      or { 'plain', 'regex', 'fuzzy' }

    -- When regex compilation failed and we fell back to literal search, show a warning
    local fallback_label = nil
    if M.state.grep_regex_fallback_error then fallback_label = 'invalid regex, using literal' end

    -- If only one mode configured and no fallback error, hide the mode indicator completely
    if #modes <= 1 and not fallback_label then
      -- Clear any existing status and don't show anything
      vim.api.nvim_buf_clear_namespace(M.state.input_buf, M.state.ns_id, 0, -1)
      M.state.last_status_info = nil
      return
    end

    -- Grep mode: show keybind + label, e.g. "<S-Tab> fuzzy"
    local keybind = config.keymaps.toggle_grep_regex
    -- Normalize: if it's a table of keys, use the first one for display
    if type(keybind) == 'table' then keybind = keybind[1] or '<S-Tab>' end

    local mode_labels = {
      plain = 'plain',
      regex = 'regex',
      fuzzy = 'fuzzy',
    }
    local mode_label = mode_labels[M.state.grep_mode] or 'plain'
    local hl
    if M.state.grep_mode == 'plain' then
      hl = config.hl.grep_regex_inactive or 'Comment'
    elseif M.state.grep_mode == 'regex' then
      hl = config.hl.grep_regex_active or 'DiagnosticInfo'
    else -- fuzzy
      hl = config.hl.grep_fuzzy_active or 'DiagnosticHint'
    end

    local cache_key = keybind .. M.state.grep_mode .. (fallback_label or '')
    if cache_key == M.state.last_status_info then return end
    M.state.last_status_info = cache_key

    vim.api.nvim_buf_clear_namespace(M.state.input_buf, M.state.ns_id, 0, -1)

    local win_width = vim.api.nvim_win_get_width(M.state.input_win)
    local available_width = win_width - 2

    local virt_text
    if fallback_label then
      local total_len = #fallback_label
      local col_position = available_width - total_len
      virt_text = { { fallback_label, 'DiagnosticWarn' } }
      vim.api.nvim_buf_set_extmark(M.state.input_buf, M.state.ns_id, 0, 0, {
        virt_text = virt_text,
        virt_text_win_col = col_position,
      })
    else
      local total_len = #keybind + 1 + #mode_label
      local col_position = available_width - total_len
      vim.api.nvim_buf_set_extmark(M.state.input_buf, M.state.ns_id, 0, 0, {
        virt_text = {
          { keybind .. ' ', hl },
          { mode_label, hl },
        },
        virt_text_win_col = col_position,
      })
    end
    return
  end

  -- File picker mode: show match counts
  local status_info
  if progress and progress.is_scanning then
    status_info = string.format('Indexing files %d', progress.scanned_files_count)
  else
    local search_metadata = file_picker.get_search_metadata()
    if #M.state.query < 2 then
      status_info = string.format('%d', search_metadata.total_files)
    else
      status_info = string.format('%d/%d', search_metadata.total_matched, search_metadata.total_files)
    end
  end

  if status_info == M.state.last_status_info then return end
  M.state.last_status_info = status_info

  vim.api.nvim_buf_clear_namespace(M.state.input_buf, M.state.ns_id, 0, -1)

  local win_width = vim.api.nvim_win_get_width(M.state.input_win)
  local available_width = win_width - 2
  local col_position = available_width - #status_info

  vim.api.nvim_buf_set_extmark(M.state.input_buf, M.state.ns_id, 0, 0, {
    virt_text = { { status_info, 'LineNr' } },
    virt_text_win_col = col_position,
  })
end

function M.move_up()
  if not M.state.active then return end
  if #M.state.filtered_items == 0 then return end

  local prompt_position = get_prompt_position()
  local items_count = #M.state.filtered_items

  -- Pagination logic depends on prompt position
  if prompt_position == 'bottom' then
    -- Bottom prompt with reverse rendering: visually moving UP means cursor INCREASES
    -- because higher index items are rendered at lower line numbers
    local near_bottom = M.state.cursor >= (items_count - M.state.pagination.prefetch_margin)
    local at_last_item = M.state.cursor >= items_count

    if near_bottom and at_last_item then
      local page_size = M.state.pagination.page_size
      if page_size > 0 then
        local has_more
        if M.state.mode == 'grep' then
          has_more = M.state.pagination.grep_next_file_offset > 0
        else
          local max_page = math.max(0, math.ceil(M.state.pagination.total_matched / page_size) - 1)
          has_more = M.state.pagination.page_index < max_page
        end
        if has_more then
          M.load_next_page()
          return
        end
      end
    end

    M.state.cursor = math.min(M.state.cursor + 1, items_count)
  else
    -- Top prompt: scrolling UP means going to BETTER results (previous page)
    if M.state.cursor <= M.state.pagination.prefetch_margin + 1 and M.state.cursor <= 1 then
      if M.state.pagination.page_index > 0 then
        vim.schedule(M.load_previous_page)
        return
      end
    end

    M.state.cursor = math.max(M.state.cursor - 1, 1)
  end

  M.render_list()
  if M.state.mode == 'grep' or M.state.suggestion_source == 'grep' then
    M.update_preview_smart()
  else
    M.update_preview()
  end
  M.update_status()

  if M.state.combo_initial_cursor and M.state.combo_visible then
    local cursor_distance = math.abs(M.state.cursor - M.state.combo_initial_cursor)
    local half_page = math.floor(M.state.pagination.page_size / 2)
    if cursor_distance > half_page then
      M.state.combo_visible = false
      combo_renderer.cleanup()
      M.render_list() -- Re-render once without combo
      -- Scroll to bottom for bottom prompt to eliminate gap
      if get_prompt_position() == 'bottom' then scroll_to_bottom() end
    end
  end
end

function M.move_down()
  if not M.state.active then return end
  if #M.state.filtered_items == 0 then return end

  local prompt_position = get_prompt_position()
  local items_count = #M.state.filtered_items

  -- Pagination logic depends on prompt position
  if prompt_position == 'bottom' then
    -- Bottom prompt with reverse rendering: visually moving DOWN means cursor DECREASES
    -- because lower index items (better) are rendered at higher line numbers
    if M.state.cursor <= M.state.pagination.prefetch_margin + 1 and M.state.cursor <= 1 then
      if M.state.pagination.page_index > 0 then
        vim.schedule(M.load_previous_page)
        return
      end
    end

    M.state.cursor = math.max(M.state.cursor - 1, 1)
  else
    -- Top prompt: scrolling DOWN means going to WORSE results (next page)
    local near_bottom = M.state.cursor >= (items_count - M.state.pagination.prefetch_margin)
    local at_last_item = M.state.cursor >= items_count

    if near_bottom and at_last_item then
      local page_size = M.state.pagination.page_size
      if page_size > 0 then
        local has_more
        if M.state.mode == 'grep' then
          has_more = M.state.pagination.grep_next_file_offset > 0
        else
          local max_page = math.max(0, math.ceil(M.state.pagination.total_matched / page_size) - 1)
          has_more = M.state.pagination.page_index < max_page
        end
        if has_more then
          M.load_next_page()
          return
        end
      end
    end

    M.state.cursor = math.min(M.state.cursor + 1, items_count)
  end

  M.render_list()
  if M.state.mode == 'grep' or M.state.suggestion_source == 'grep' then
    M.update_preview_smart()
  else
    M.update_preview()
  end
  M.update_status()

  if M.state.combo_initial_cursor and M.state.combo_visible then
    local cursor_distance = math.abs(M.state.cursor - M.state.combo_initial_cursor)
    local half_page = math.floor(M.state.pagination.page_size / 2)
    if cursor_distance > half_page then
      M.state.combo_visible = false
      combo_renderer.cleanup()
      M.render_list() -- Re-render once without combo
      -- Scroll to bottom for bottom prompt to eliminate gap
      if get_prompt_position() == 'bottom' then scroll_to_bottom() end
    end
  end
end

--- Scroll preview up by half window height
function M.scroll_preview_up()
  if not M.state.active or not M.state.preview_win then return end

  local win_height = vim.api.nvim_win_get_height(M.state.preview_win)
  local scroll_lines = math.floor(win_height / 2)

  preview.scroll(-scroll_lines)
end

--- Scroll preview down by half window height
function M.scroll_preview_down()
  if not M.state.active or not M.state.preview_win then return end

  local win_height = vim.api.nvim_win_get_height(M.state.preview_win)
  local scroll_lines = math.floor(win_height / 2)

  preview.scroll(scroll_lines)
end

--- Reset history cycling state
function M.reset_history_state()
  M.state.history_offset = nil
  M.state.updating_from_history = false
end

--- Recall query from history with temporary min_combo_count=0
function M.recall_query_from_history()
  if not M.state.active then return end

  -- Initialize offset on first press
  if M.state.history_offset == nil then
    M.state.history_offset = 0
  else
    -- Increment offset for next query
    M.state.history_offset = M.state.history_offset + 1
  end

  -- Fetch query at current offset from Rust (grep and file picker have separate histories)
  local fuzzy = require('fff.core').ensure_initialized()
  local history_fn = M.state.mode == 'grep' and fuzzy.get_historical_grep_query or fuzzy.get_historical_query
  local ok, query = pcall(history_fn, M.state.history_offset)

  if not ok or not query then
    -- Reached end of history, wrap to beginning
    M.state.history_offset = 0
    ok, query = pcall(history_fn, 0)

    if not ok or not query then
      -- No history available at all
      vim.notify('No query history available', vim.log.levels.INFO)
      M.state.history_offset = nil
      return
    end
  end

  if M.state.mode ~= 'grep' then M.state.next_search_force_combo_boost = true end

  -- this is going to trigger the on_input_change handler with the normal search and render flow
  vim.api.nvim_buf_set_lines(M.state.input_buf, 0, -1, false, { M.state.config.prompt .. query })

  -- Position cursor at end
  vim.schedule(function()
    if M.state.active and M.state.input_win and vim.api.nvim_win_is_valid(M.state.input_win) then
      vim.api.nvim_win_set_cursor(M.state.input_win, { 1, #M.state.config.prompt + #query })
    end
  end)
end

--- Find the first visible window with a normal file buffer
--- @return number|nil Window ID of the first suitable window, or nil if none found
local function find_suitable_window()
  local current_tabpage = vim.api.nvim_get_current_tabpage()
  local windows = vim.api.nvim_tabpage_list_wins(current_tabpage)

  for _, win in ipairs(windows) do
    if vim.api.nvim_win_is_valid(win) then
      local buf = vim.api.nvim_win_get_buf(win)
      if vim.api.nvim_buf_is_valid(buf) then
        local buftype = vim.api.nvim_buf_get_option(buf, 'buftype')
        local modifiable = vim.api.nvim_buf_get_option(buf, 'modifiable')
        local filetype = vim.api.nvim_buf_get_option(buf, 'filetype')

        local is_picker_window = (
          win == M.state.input_win
          or win == M.state.list_win
          or win == M.state.preview_win
          or win == M.state.file_info_win
        )

        if
          (buftype == '' or buftype == 'acwrite')
          and modifiable
          and not is_picker_window
          and filetype ~= 'undotree'
        then
          return win
        end
      end
    end
  end

  return nil
end

--- Build a unique key for a grep match occurrence.
--- Format: "path:line:col" — uniquely identifies one match entry.
---@param item table Grep match item with path, line_number, col
---@return string
local function grep_item_key(item) return string.format('%s:%d:%d', item.path, item.line_number or 0, item.col or 0) end

--- Toggle selection for the current item.
--- In grep mode, selection is per-occurrence (individual match line).
--- In file mode, selection is per-file path.
function M.toggle_select()
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then return end

  local item = items[M.state.cursor]
  if not item or not item.path then return end

  local was_selected

  if M.state.mode == 'grep' then
    -- Per-occurrence selection for grep mode
    local key = grep_item_key(item)
    was_selected = M.state.selected_items[key] ~= nil
    if was_selected then
      M.state.selected_items[key] = nil
    else
      M.state.selected_items[key] = item
    end
  else
    -- Per-file selection for normal file mode
    was_selected = M.state.selected_files[item.path]
    if was_selected then
      M.state.selected_files[item.path] = nil
    else
      M.state.selected_files[item.path] = true
    end
  end

  M.render_list()

  -- only when selecting the element not deselecting
  if not was_selected then
    if get_prompt_position() == 'bottom' then
      M.move_up()
    else
      M.move_down()
    end
  end
end

--- Send selected files/matches to quickfix list and close picker.
--- Normal file mode: entries at line 1, col 1.
--- Grep mode with selections: selected occurrences with exact line/col.
--- Grep mode without selections: re-runs search with large limit to collect all matches.
function M.send_to_quickfix()
  if not M.state.active then return end

  local qf_list = {}

  if M.state.mode == 'grep' then
    -- Grep mode: per-occurrence entries with exact locations
    local has_selections = next(M.state.selected_items) ~= nil

    if has_selections then
      -- Use explicitly selected items (survives page changes)
      for _, item in pairs(M.state.selected_items) do
        table.insert(qf_list, {
          filename = item.path,
          lnum = item.line_number or 1,
          col = (item.col or 0) + 1,
          text = item.line_content or vim.fn.fnamemodify(item.path, ':.'),
        })
      end
    else
      -- No selections: run an exhaustive search to get all matches
      local grep = require('fff.grep')
      local exhaustive = grep.search(M.state.query, 0, 10000, M.state.grep_config, M.state.grep_mode)
      local all_items = exhaustive and exhaustive.items or {}

      if #all_items == 0 then
        vim.notify('No matches to send to quickfix', vim.log.levels.WARN)
        return
      end

      for _, item in ipairs(all_items) do
        if item and item.path then
          table.insert(qf_list, {
            filename = item.path,
            lnum = item.line_number or 1,
            col = (item.col or 0) + 1,
            text = item.line_content or vim.fn.fnamemodify(item.path, ':.'),
          })
        end
      end
    end
  else
    -- Normal file mode: per-file entries at line 1
    local paths = {}

    -- Collect from explicit selections, or fall back to all visible items
    if next(M.state.selected_files) then
      for path, _ in pairs(M.state.selected_files) do
        table.insert(paths, path)
      end
    else
      for _, item in ipairs(M.state.filtered_items) do
        if item and item.path then table.insert(paths, item.path) end
      end
    end

    if #paths == 0 then
      vim.notify('No files to send to quickfix', vim.log.levels.WARN)
      return
    end

    for _, path in ipairs(paths) do
      table.insert(qf_list, {
        filename = path,
        lnum = 1,
        col = 1,
        text = vim.fn.fnamemodify(path, ':.'),
      })
    end
  end

  -- Close picker first, then populate quickfix
  local is_grep = M.state.mode == 'grep'
  M.close()

  vim.fn.setqflist(qf_list, 'r')
  vim.cmd('copen')

  local count = #qf_list
  local unit = is_grep and (count == 1 and 'match' or 'matches') or (count == 1 and 'file' or 'files')
  vim.notify(string.format('Added %d %s to quickfix list', count, unit), vim.log.levels.INFO)
end

function M.select(action)
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then return end

  local item = items[M.state.cursor]
  if not item then return end

  action = action or 'edit'

  -- Strip Windows long path prefix (\\?\) if present.
  -- These can surface from Rust's fs::canonicalize on Windows when LongPathsEnabled is set.
  -- Neovim cannot open paths with this prefix. The Rust side uses dunce::canonicalize to avoid
  -- producing these, but we strip defensively here as well.
  local path = item.path
  if vim.startswith(path, '\\\\?\\') then path = path:sub(5) end

  local relative_path = vim.fn.fnamemodify(path, ':.')
  local location = M.state.location -- Capture location before closing
  local query = M.state.query -- Capture query before closing for tracking
  local mode = M.state.mode -- Capture mode before closing for tracking
  local suggestion_source = M.state.suggestion_source -- Capture suggestion context

  -- In grep mode (or when selecting a grep suggestion), derive location from the match item
  local is_grep_item = mode == 'grep' or suggestion_source == 'grep'
  if is_grep_item and item.line_number and item.line_number > 0 then
    location = { line = item.line_number }
    if item.col and item.col > 0 then
      location.col = item.col + 1 -- Convert 0-based byte col to 1-based
    end
  end

  vim.cmd('stopinsert')
  M.close()

  if action == 'edit' then
    local current_buf = vim.api.nvim_get_current_buf()
    local current_buftype = vim.api.nvim_buf_get_option(current_buf, 'buftype')
    local current_buf_modifiable = vim.api.nvim_buf_get_option(current_buf, 'modifiable')

    -- If current active buffer is not a normal buffer we find a suitable window with a tab otherwise opening a new split
    if current_buftype ~= '' or not current_buf_modifiable then
      local suitable_win = find_suitable_window()
      if suitable_win then vim.api.nvim_set_current_win(suitable_win) end
    end

    vim.cmd('edit ' .. vim.fn.fnameescape(relative_path))
  elseif action == 'split' then
    vim.cmd('split ' .. vim.fn.fnameescape(relative_path))
  elseif action == 'vsplit' then
    vim.cmd('vsplit ' .. vim.fn.fnameescape(relative_path))
  elseif action == 'tab' then
    vim.cmd('tabedit ' .. vim.fn.fnameescape(relative_path))
  end

  -- Derive side effects on vim schedule to ensure they run after the file is opened
  vim.schedule(function()
    if location then location_utils.jump_to_location(location) end

    if query and query ~= '' then
      local config = conf.get()
      if config.history and config.history.enabled then
        local fff = require('fff.core').ensure_initialized()
        -- Track in background thread (non-blocking, handled by Rust)
        if mode == 'grep' then
          pcall(fff.track_grep_query, query)
        else
          pcall(fff.track_query_completion, query, item.path)
        end
      end
    end
  end)
end

function M.close()
  if not M.state.active then return end

  vim.cmd('stopinsert')
  M.state.active = false

  combo_renderer.cleanup()
  scrollbar.cleanup()

  -- Clean up treesitter scratch buffers used for grep syntax highlighting
  local ts_ok, ts_hl = pcall(require, 'fff.treesitter_hl')
  if ts_ok then ts_hl.cleanup() end

  local windows = {
    M.state.input_win,
    M.state.list_win,
    M.state.preview_win,
  }

  if M.state.file_info_win then table.insert(windows, M.state.file_info_win) end

  for _, win in ipairs(windows) do
    if win and vim.api.nvim_win_is_valid(win) then vim.api.nvim_win_close(win, true) end
  end

  local buffers = {
    M.state.input_buf,
    M.state.list_buf,
    M.state.file_info_buf,
  }
  if M.enabled_preview() then buffers[#buffers + 1] = M.state.preview_buf end

  for _, buf in ipairs(buffers) do
    if buf and vim.api.nvim_buf_is_valid(buf) then
      vim.api.nvim_buf_clear_namespace(buf, -1, 0, -1)

      if buf == M.state.preview_buf then preview.clear_buffer(buf) end

      vim.api.nvim_buf_delete(buf, { force = true })
    end
  end

  if M.state.preview_timer then
    M.state.preview_timer:stop()
    M.state.preview_timer:close()
    M.state.preview_timer = nil
  end

  M.state.input_win = nil
  M.state.list_win = nil
  M.state.file_info_win = nil
  M.state.preview_win = nil
  M.state.input_buf = nil
  M.state.list_buf = nil
  M.state.file_info_buf = nil
  M.state.preview_buf = nil
  M.state.items = {}
  M.state.filtered_items = {}
  M.state.cursor = 1
  M.state.query = ''
  M.state.ns_id = nil
  M.state.last_preview_file = nil
  M.state.last_preview_location = nil
  M.state.current_file_cache = nil
  M.state.location = nil
  M.state.selected_files = {}
  M.state.selected_items = {}
  M.state.mode = nil
  M.state.grep_config = nil
  M.state.grep_mode = 'plain'
  M.state.grep_regex_fallback_error = nil
  M.state.suggestion_items = nil
  M.state.suggestion_source = nil
  M.state.combo_visible = true
  M.state.combo_initial_cursor = nil
  M.reset_history_state()
  -- Clean up picker focus autocmds
  pcall(vim.api.nvim_del_augroup_by_name, 'fff_picker_focus')
end

--- Helper function to determine current file cache for deprioritization
--- @param base_path string Base path for relative path calculation
--- @return string|nil Current file cache path
local function get_current_file_cache(base_path)
  local current_buf = vim.api.nvim_get_current_buf()
  if not current_buf or not vim.api.nvim_buf_is_valid(current_buf) then return nil end

  local current_file = vim.api.nvim_buf_get_name(current_buf)
  if current_file == '' then return nil end

  -- Use vim.uv.fs_stat to check if file exists and is readable
  local stat = vim.uv.fs_stat(current_file)
  if not stat or stat.type ~= 'file' then return nil end

  local absolute_path = vim.fn.fnamemodify(current_file, ':p')
  local resolved_abs = vim.fn.resolve(absolute_path)
  local resolved_base = vim.fn.resolve(base_path)

  -- icloud direcrtoes on macos contain a lot of special characters that break
  -- the fnamemodify which have to escaped with %
  local escaped_base = resolved_base:gsub('([%%^$()%.%[%]*+%-?])', '%%%1')
  local relative_path = resolved_abs:gsub('^' .. escaped_base .. '/', '')
  if relative_path == '' or relative_path == resolved_abs then return nil end
  return relative_path
end

--- Helper function for common picker initialization
--- @param opts table|nil Options passed to the picker
--- @return table|nil Merged configuration, nil if initialization failed
local function initialize_picker(opts)
  local base_path = opts and opts.cwd or vim.uv.cwd()

  -- Initialize file picker if needed
  if not file_picker.is_initialized() then
    if not file_picker.setup() then
      vim.notify('Failed to initialize file picker', vim.log.levels.ERROR)
      return nil
    end
  end

  local config = conf.get()
  local merged_config = vim.tbl_deep_extend('force', config or {}, opts or {})

  return merged_config, base_path
end

--- Helper function to open UI with optional prefetched results
--- @param query string|nil Pre-filled query (nil for empty)
--- @param results table|nil Pre-fetched results (nil to search normally)
--- @param location table|nil Pre-fetched location data
--- @param merged_config table Merged configuration
--- @param current_file_cache string|nil Current file cache
local function open_ui_with_state(query, results, location, merged_config, current_file_cache)
  M.state.config = merged_config

  if not M.create_ui() then
    vim.notify('Failed to create picker UI', vim.log.levels.ERROR)
    return false
  end

  M.state.active = true
  M.state.current_file_cache = current_file_cache

  -- Set up initial state
  if query then
    M.state.query = query
    vim.api.nvim_buf_set_lines(M.state.input_buf, 0, -1, false, { M.state.config.prompt .. query })
  else
    M.state.query = ''
  end

  if results then
    -- Use prefetched results
    M.state.items = results
    M.state.filtered_items = results
    M.state.cursor = #results > 0 and 1 or 1
    M.state.location = location

    M.render_list()
    M.update_preview()
    M.update_status()
  else
    M.update_results()
    M.clear_preview()
    M.update_status()
  end

  vim.api.nvim_set_current_win(M.state.input_win)

  -- Position cursor at end of query if there is one
  if query then
    vim.schedule(function()
      if M.state.active and M.state.input_win and vim.api.nvim_win_is_valid(M.state.input_win) then
        vim.api.nvim_win_set_cursor(M.state.input_win, { 1, #M.state.config.prompt + #query })
        vim.cmd('startinsert!')
      end
    end)
  else
    vim.cmd('startinsert!')
  end

  M.monitor_scan_progress(0)
  return true
end

--- Execute a search query with callback handling before potentially opening the UI
--- @param query string The search query to execute
--- @param callback function Function called with results: function(results, metadata, location, get_file_score) -> boolean
--- @param opts? table Optional configuration to override defaults (same as M.open)
--- @return boolean true if callback handled results, false if UI was opened
function M.open_with_callback(query, callback, opts)
  if M.state.active then return false end

  local merged_config, base_path = initialize_picker(opts)
  if not merged_config then return false end

  local current_file_cache = get_current_file_cache(base_path)
  local results = file_picker.search_files(query, nil, nil, current_file_cache, nil)

  local metadata = file_picker.get_search_metadata()
  local location = file_picker.get_search_location()

  local callback_handled = false
  if type(callback) == 'function' then
    local ok, result = pcall(callback, results, metadata, location, file_picker.get_file_score)
    if ok then
      callback_handled = result == true
    else
      vim.notify('Error in search callback: ' .. tostring(result), vim.log.levels.ERROR)
    end
  end

  if callback_handled then return true end
  open_ui_with_state(query, results, location, merged_config, current_file_cache)

  return false
end

--- Open the file picker UI
--- @param opts? table Optional configuration to override defaults
--- @param opts.cwd? string Custom working directory (default: vim.fn.getcwd())
--- @param opts.title? string Window title (default: "FFFiles")
--- @param opts.prompt? string Input prompt text (default: "🪿 ")
--- @param opts.max_results? number Maximum number of results to display (default: 100)
--- @param opts.max_threads? number Maximum number of threads for file scanning (default: 4)
--- @param opts.layout? table Layout configuration
--- @param opts.layout.width? number|function Window width as ratio (0.0-1.0) or function(terminal_width, terminal_height): number (default: 0.8)
--- @param opts.layout.height? number|function Window height as ratio (0.0-1.0) or function(terminal_width, terminal_height): number (default: 0.8)
--- @param opts.layout.prompt_position? string|function Prompt position: 'top'|'bottom' or function(terminal_width, terminal_height): string (default: 'bottom')
--- @param opts.layout.preview_position? string|function Preview position: 'left'|'right'|'top'|'bottom' or function(terminal_width, terminal_height): string (default: 'right')
--- @param opts.layout.preview_size? number|function Preview size as ratio (0.0-1.0) or function(terminal_width, terminal_height): number (default: 0.5)
--- @param opts.renderer? table Custom renderer implementing {render_line, apply_highlights} interface (default: file_renderer)
--- @param opts.mode? string Picker mode: nil (default file picker) or 'grep' (live grep)
--- @param opts.grep_config? table Grep-specific config overrides {max_file_size, smart_case, max_matches_per_file}
function M.open(opts)
  if M.state.active then return end

  M.state.selected_files = {}
  M.state.selected_items = {}
  M.state.renderer = opts and opts.renderer or nil
  M.state.mode = opts and opts.mode or nil
  M.state.grep_config = opts and opts.grep_config or nil

  local merged_config, base_path = initialize_picker(opts)
  if not merged_config then return end

  -- Initialize grep_mode to first configured mode when opening in grep mode
  if M.state.mode == 'grep' then
    -- Use grep_config.modes if provided, otherwise fall back to global config
    local modes = (M.state.grep_config and M.state.grep_config.modes)
      or merged_config.grep.modes
      or { 'plain', 'regex', 'fuzzy' }
    M.state.grep_mode = modes[1] or 'plain'
  end

  local current_file_cache = get_current_file_cache(base_path)
  local query = opts and opts.query or nil
  return open_ui_with_state(query, nil, nil, merged_config, current_file_cache)
end

function M.monitor_scan_progress(iteration)
  if not M.state.active then return end

  local progress = file_picker.get_scan_progress()

  if progress.is_scanning then
    M.update_status(progress)

    local timeout
    if iteration < 10 then
      timeout = 100
    elseif iteration < 20 then
      timeout = 300
    else
      timeout = 500
    end

    vim.defer_fn(function() M.monitor_scan_progress(iteration + 1) end, timeout)
  else
    M.update_results()
  end
end

M.enabled_preview = function()
  local preview_state = nil

  if M and M.state and M.state.config then preview_state = M.state.config.preview end
  if not preview_state then return true end

  return preview_state.enabled
end

return M
