local M = {}

local conf = require('fff.conf')
local file_picker = require('fff.file_picker')
local preview = require('fff.file_picker.preview')
local icons = require('fff.file_picker.icons')
local git_utils = require('fff.git_utils')
local utils = require('fff.utils')
local location_utils = require('fff.location_utils')
local combo_renderer = require('fff.combo_renderer')

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

  config = nil,

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
}

function M.create_ui()
  local config = M.state.config

  if not M.state.ns_id then
    M.state.ns_id = vim.api.nvim_create_namespace('fff_picker_status')
    combo_renderer.init(M.state.ns_id)
  end

  local debug_enabled_in_preview = M.enabled_preview() and config and config.debug and config.debug.show_scores

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

  -- Create list window with conditional title based on prompt position
  local list_window_config = {
    relative = 'editor',
    width = layout.list_width,
    height = layout.list_height,
    col = layout.list_col,
    row = layout.list_row,
    -- To make the input feel connected with the picker, we customize the
    -- respective corner border characters based on prompt_position
    border = prompt_position == 'bottom' and { '┌', '─', '┐', '│', '', '', '', '│' }
      or { '├', '─', '┤', '│', '┘', '─', '└', '│' },
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
      border = 'single',
      style = 'minimal',
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
      border = 'single',
      style = 'minimal',
      title = ' Preview ',
      title_pos = 'left',
    })
  end

  -- Create input window with conditional title based on prompt position
  local input_window_config = {
    relative = 'editor',
    width = layout.input_width,
    height = 1,
    col = layout.input_col,
    row = layout.input_row,
    -- To make the input feel connected with the picker, we customize the
    -- respective corner border characters based on prompt_position
    border = prompt_position == 'bottom' and { '├', '─', '┤', '│', '┘', '─', '└', '│' }
      or { '┌', '─', '┐', '│', '', '', '', '│' },
    style = 'minimal',
  }

  -- Add title if prompt is at top - title appears above the prompt
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
    vim.api.nvim_win_set_option(M.state.preview_win, 'cursorline', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'number', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'relativenumber', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'signcolumn', 'no')
    vim.api.nvim_win_set_option(M.state.preview_win, 'foldcolumn', '0')
    vim.api.nvim_win_set_option(M.state.preview_win, 'winhighlight', win_hl)
  end

  local picker_group = vim.api.nvim_create_augroup('fff_picker_focus', { clear = true })
  local picker_windows = nil

  if M.enabled_preview() then
    picker_windows = { M.state.input_win, M.state.preview_win, M.state.list_win }
  else
    picker_windows = { M.state.input_win, M.state.list_win }
  end

  if M.state.preview_win then table.insert(picker_windows, M.state.preview_win) end
  if M.state.file_info_win then table.insert(picker_windows, M.state.file_info_win) end

  vim.api.nvim_create_autocmd('WinLeave', {
    group = picker_group,
    callback = function()
      if not M.state.active then return end

      local current_win = vim.api.nvim_get_current_win()
      local is_picker_window = false
      for _, win in ipairs(picker_windows) do
        if win and vim.api.nvim_win_is_valid(win) and current_win == win then
          is_picker_window = true
          break
        end
      end

      -- if we current focused on picker window and leaving it
      if is_picker_window then
        vim.defer_fn(function()
          if not M.state.active then return end

          local new_win = vim.api.nvim_get_current_win()
          local entering_picker_window = false

          for _, win in ipairs(picker_windows) do
            if win and vim.api.nvim_win_is_valid(win) and new_win == win then
              entering_picker_window = true
              break
            end
          end

          if not entering_picker_window then M.close() end
        end, 10)
      end
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

function M.setup_keymaps()
  local keymaps = M.state.config.keymaps

  local input_opts = { buffer = M.state.input_buf, noremap = true, silent = true }

  set_keymap('i', keymaps.close, M.close, input_opts)
  set_keymap('i', keymaps.select, M.select, input_opts)
  set_keymap('i', keymaps.select_split, function() M.select('split') end, input_opts)
  set_keymap('i', keymaps.select_vsplit, function() M.select('vsplit') end, input_opts)
  set_keymap('i', keymaps.select_tab, function() M.select('tab') end, input_opts)
  set_keymap('i', keymaps.move_up, M.move_up, input_opts)
  set_keymap('i', keymaps.move_down, M.move_down, input_opts)
  set_keymap('i', keymaps.preview_scroll_up, M.scroll_preview_up, input_opts)
  set_keymap('i', keymaps.preview_scroll_down, M.scroll_preview_down, input_opts)
  set_keymap('i', keymaps.toggle_debug, M.toggle_debug, input_opts)
  set_keymap('i', keymaps.cycle_previous_query, M.recall_query_from_history, input_opts)
  set_keymap('i', keymaps.toggle_select, M.toggle_select, input_opts)
  set_keymap('i', keymaps.send_to_quickfix, M.send_to_quickfix, input_opts)

  local list_opts = { buffer = M.state.list_buf, noremap = true, silent = true }

  set_keymap('n', keymaps.close, M.focus_input_win, list_opts)
  set_keymap('n', keymaps.select, M.select, list_opts)
  set_keymap('n', keymaps.select_split, function() M.select('split') end, list_opts)
  set_keymap('n', keymaps.select_vsplit, function() M.select('vsplit') end, list_opts)
  set_keymap('n', keymaps.select_tab, function() M.select('tab') end, list_opts)
  set_keymap('n', keymaps.move_up, M.move_up, list_opts)
  set_keymap('n', keymaps.move_down, M.move_down, list_opts)
  set_keymap('n', keymaps.preview_scroll_up, M.scroll_preview_up, list_opts)
  set_keymap('n', keymaps.preview_scroll_down, M.scroll_preview_down, list_opts)
  set_keymap('n', keymaps.toggle_debug, M.toggle_debug, list_opts)
  set_keymap('n', keymaps.toggle_select, M.toggle_select, list_opts)
  set_keymap('n', keymaps.send_to_quickfix, M.send_to_quickfix, list_opts)

  if M.state.preview_buf then
    local preview_opts = { buffer = M.state.preview_buf, noremap = true, silent = true }

    set_keymap('n', keymaps.close, M.focus_input_win, preview_opts)
    set_keymap('n', keymaps.select, M.select, preview_opts)
    set_keymap('n', keymaps.select_split, function() M.select('split') end, preview_opts)
    set_keymap('n', keymaps.select_vsplit, function() M.select('vsplit') end, preview_opts)
    set_keymap('n', keymaps.select_tab, function() M.select('tab') end, preview_opts)
    set_keymap('n', keymaps.toggle_debug, M.toggle_debug, preview_opts)
    set_keymap('n', keymaps.toggle_select, M.toggle_select, preview_opts)
    set_keymap('n', keymaps.send_to_quickfix, M.send_to_quickfix, preview_opts)
  end

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

    return '' -- Return empty string to prevent default <C-w> behavior
  end, input_opts)

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

    M.close()
    M.open()

    M.state.query = current_query
    M.state.items = current_items
    M.state.cursor = current_cursor
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

--- Handle input change
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

  -- Calculate dynamic max_results based on visible window height
  local dynamic_max_results = M.state.config.max_results
  if M.state.list_win and vim.api.nvim_win_is_valid(M.state.list_win) then
    local win_height = vim.api.nvim_win_get_height(M.state.list_win)
    dynamic_max_results = win_height
  else
    dynamic_max_results = M.state.config.max_results or 100
  end

  -- Check if we should force combo boost for this search (history recall)
  local min_combo_override = nil
  if M.state.next_search_force_combo_boost then
    min_combo_override = 0 -- Force combo boost by setting min_combo_count to 0
  end

  local results = file_picker.search_files(
    M.state.query,
    M.state.current_file_cache,
    dynamic_max_results,
    M.state.config.max_threads,
    prompt_position == 'bottom',
    min_combo_override
  )

  -- Get location from search results
  M.state.location = file_picker.get_search_location()

  -- because the actual files could be different even with same count
  M.state.items = results
  M.state.filtered_items = results

  if prompt_position == 'bottom' then
    M.state.cursor = #results > 0 and #results or 1
  else
    M.state.cursor = 1
  end

  M.render_debounced()
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
  if #path <= max_width then return path end

  local segments = {}
  for segment in path:gmatch('[^/]+') do
    table.insert(segments, segment)
  end

  if #segments <= 2 then
    return path -- Can't shrink further
  end

  local first = segments[1]
  local last = segments[#segments]
  local ellipsis = '../'

  for middle_count = #segments - 2, 1, -1 do
    local middle_parts = {}
    local start_idx = 2
    local end_idx = math.min(start_idx + middle_count - 1, #segments - 1)

    for i = start_idx, end_idx do
      table.insert(middle_parts, segments[i])
    end

    local middle = table.concat(middle_parts, '/')
    if middle_count < #segments - 2 then middle = middle .. ellipsis end

    local result = first .. '/' .. middle .. '/' .. last
    if #result <= max_width then return result end
  end

  return first .. '/' .. ellipsis .. last
end

local function format_file_display(item, max_width)
  local filename = item.name
  local dir_path = item.directory or ''

  if dir_path == '' and item.relative_path then
    local parent_dir = vim.fn.fnamemodify(item.relative_path, ':h')
    if parent_dir ~= '.' and parent_dir ~= '' then dir_path = parent_dir end
  end

  local base_width = #filename + 1 -- filename + " "
  local path_max_width = max_width - base_width

  if dir_path == '' then return filename, '' end
  local display_path = shrink_path(dir_path, path_max_width)

  return filename, display_path
end

--- Calculate number of rows an item will occupy when rendered
--- @param item_index number Index of the item (1-based)
--- @param has_combo boolean Whether any combo boost exists
--- @param combo_item_index number|nil Index of the combo-boosted item
--- @return number Number of rows (1 or 2 currently)
local function get_item_row_count(item_index, has_combo, combo_item_index)
  if has_combo and item_index == combo_item_index then
    return 2 -- Combo header line + content line
  end
  return 1 -- Just content line
end

function M.render_list()
  if not M.state.active then return end

  local config = conf.get()
  local items = M.state.filtered_items
  local max_path_width = config.ui and config.ui.max_path_width or 80
  local debug_enabled = config and config.debug and config.debug.show_scores
  local win_height = vim.api.nvim_win_get_height(M.state.list_win)
  local win_width = vim.api.nvim_win_get_width(M.state.list_win)
  local empty_lines_needed = 0

  local combo_boost_score_multiplier = config.history and config.history.combo_boost_score_multiplier or 100
  local has_combo, combo_header_line, combo_header_text_len, combo_item_index = combo_renderer.detect_and_prepare(
    items,
    file_picker,
    win_width,
    combo_boost_score_multiplier,
    -- disable rendering of combos if cycling through history or user wants to always show the last match
    M.state.next_search_force_combo_boost or config.history.min_combo_count == 0
  )
  M.state.next_search_force_combo_boost = false -- effectively reset if set by the history recall

  -- Calculate how many items fit (accounting for multi-row items)
  local display_count = 0
  local accumulated_rows = 0
  if #items > 0 then
    display_count = 1 -- Always show at least first item, even if it exceeds win_height
    accumulated_rows = get_item_row_count(1, has_combo, combo_item_index)

    for i = 2, #items do
      local item_rows = get_item_row_count(i, has_combo, combo_item_index)
      if accumulated_rows + item_rows > win_height then
        break -- Next item won't fit
      end
      accumulated_rows = accumulated_rows + item_rows
      display_count = i
    end
  end

  local prompt_position = get_prompt_position()

  -- Calculate which items to display based on prompt position
  local display_start = 1
  local display_end = display_count

  if prompt_position == 'bottom' and #items > display_count then
    -- Bottom prompt: show last N items (including combo if it naturally fits)
    display_end = #items
    display_start = math.max(1, display_end - display_count + 1)
  end

  display_count = display_end - display_start + 1

  if M.state.cursor < display_start then
    M.state.cursor = display_start
  elseif M.state.cursor > display_end then
    M.state.cursor = display_end
  end

  local padded_lines = {}
  local icon_data = {}
  local path_data = {}
  local item_to_lines = {} -- Maps item index to its line indices {first_line, last_line}

  for i = display_start, display_end do
    local item = items[i]
    local item_start_line = #padded_lines + 1

    -- For combo items, insert header first
    if has_combo and combo_item_index and i == combo_item_index then table.insert(padded_lines, combo_header_line) end

    local icon, icon_hl_group = icons.get_icon(item.name, item.extension, false)
    icon_data[i] = { icon, icon_hl_group }

    local frecency = ''
    if debug_enabled then
      local total_frecency = (item.total_frecency_score or 0)
      local access_frecency = (item.access_frecency_score or 0)
      local mod_frecency = (item.modification_frecency_score or 0)

      if total_frecency > 0 then
        local indicator = ''
        if mod_frecency >= 6 then
          indicator = '🔥'
        elseif access_frecency >= 4 then
          indicator = '⭐'
        elseif total_frecency >= 3 then
          indicator = '✨'
        elseif total_frecency >= 1 then
          indicator = '•'
        end
        frecency = string.format(' %s%d', indicator, total_frecency)
      end
    end

    local icon_width = icon and (vim.fn.strdisplaywidth(icon) + 1) or 0
    local available_width = math.max(max_path_width - icon_width - #frecency, 40)

    local filename, dir_path = format_file_display(item, available_width)
    path_data[i] = { filename, dir_path }

    local line = icon and string.format('%s %s %s%s', icon, filename, dir_path, frecency)
      or string.format('%s %s%s', filename, dir_path, frecency)

    local line_len = vim.fn.strdisplaywidth(line)
    local padding = math.max(0, win_width - line_len + 5)
    table.insert(padded_lines, line .. string.rep(' ', padding))

    -- Record line range for this item
    local item_end_line = #padded_lines
    item_to_lines[i] = {
      first = item_start_line,
      last = item_end_line,
    }
  end

  -- Handle bottom positioning: add empty lines at the top
  local empty_line_offset = 0
  if prompt_position == 'bottom' then
    local total_content_lines = #padded_lines
    empty_lines_needed = math.max(0, win_height - total_content_lines)

    if empty_lines_needed > 0 then
      -- Insert empty lines at the beginning
      for i = empty_lines_needed, 1, -1 do
        table.insert(padded_lines, 1, string.rep(' ', win_width + 5))
      end
      empty_line_offset = empty_lines_needed

      -- Adjust item_to_lines mapping
      for i = display_start, display_end do
        if item_to_lines[i] then
          item_to_lines[i].first = item_to_lines[i].first + empty_line_offset
          item_to_lines[i].last = item_to_lines[i].last + empty_line_offset
        end
      end
    end
  end

  -- Calculate cursor line based on current item
  local cursor_line = 0
  if #items > 0 and M.state.cursor >= 1 and M.state.cursor <= #items then
    local cursor_item = item_to_lines[M.state.cursor]
    if cursor_item then cursor_line = cursor_item.last end
  end

  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', true)
  vim.api.nvim_buf_set_lines(M.state.list_buf, 0, -1, false, padded_lines)
  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', false)

  vim.api.nvim_buf_clear_namespace(M.state.list_buf, M.state.ns_id, 0, -1)

  -- Set cursor position
  if #items > 0 and cursor_line > 0 and cursor_line <= win_height then
    vim.api.nvim_win_set_cursor(M.state.list_win, { cursor_line, 0 })
  end

  -- Apply highlighting to all items
  if #items > 0 then
    for i = display_start, display_end do
      local item = items[i]
      local item_lines = item_to_lines[i]
      if not item_lines then goto continue end

      local is_cursor_item = (M.state.cursor == i)

      -- Highlight only the content line (last line), not the combo header
      if is_cursor_item then
        local content_line = item_lines.last
        -- Highlight entire line and extend to EOL
        vim.api.nvim_buf_set_extmark(M.state.list_buf, M.state.ns_id, content_line - 1, 0, {
          end_col = 0,
          end_row = content_line,
          hl_group = M.state.config.hl.active_file,
          hl_eol = true,
          priority = 100,
        })
      end

      -- Now apply file-specific highlights to the last line
      local line_idx = item_lines.last
      local line_content = padded_lines[line_idx]

      if line_content then
        local icon, icon_hl_group = unpack(icon_data[i])
        local filename, dir_path = unpack(path_data[i])

        local score = file_picker.get_file_score(i)
        local is_current_file = score and score.current_file_penalty and score.current_file_penalty < 0

        -- Icon highlighting
        if icon and icon_hl_group and vim.fn.strdisplaywidth(icon) > 0 then
          local icon_highlight = is_current_file and 'Comment' or icon_hl_group
          vim.api.nvim_buf_add_highlight(
            M.state.list_buf,
            M.state.ns_id,
            icon_highlight,
            line_idx - 1,
            0,
            vim.fn.strdisplaywidth(icon)
          )
        end

        -- Frecency highlighting
        if debug_enabled then
          local star_start, star_end = line_content:find('⭐%d+')
          if star_start then
            vim.api.nvim_buf_add_highlight(
              M.state.list_buf,
              M.state.ns_id,
              M.state.config.hl.frecency,
              line_idx - 1,
              star_start - 1,
              star_end
            )
          end
        end

        local icon_match = line_content:match('^%S+')
        if icon_match and #filename > 0 and #dir_path > 0 then
          local prefix_len = #icon_match + 1 + #filename + 1
          vim.api.nvim_buf_add_highlight(
            M.state.list_buf,
            M.state.ns_id,
            'Comment',
            line_idx - 1,
            prefix_len,
            prefix_len + #dir_path
          )
        end

        if is_current_file then
          if not is_cursor_item then
            vim.api.nvim_buf_add_highlight(M.state.list_buf, M.state.ns_id, 'Comment', line_idx - 1, 0, -1)
          end

          local virt_text_hl = is_cursor_item and M.state.config.hl.active_file or 'Comment'
          vim.api.nvim_buf_set_extmark(M.state.list_buf, M.state.ns_id, line_idx - 1, 0, {
            virt_text = { { ' (current)', virt_text_hl } },
            virt_text_pos = 'right_align',
          })
        end

        if item.git_status and git_utils.should_show_border(item.git_status) then
          local border_char = git_utils.get_border_char(item.git_status)
          local border_hl

          if is_cursor_item then
            -- When selected, create a combined highlight: border color on cursor background
            local base_hl = git_utils.get_border_highlight(item.git_status)
            if base_hl and base_hl ~= '' then
              -- Get the foreground color from the border highlight
              local border_fg = vim.fn.synIDattr(vim.fn.synIDtrans(vim.fn.hlID(base_hl)), 'fg')
              -- Get the background from cursor highlight
              local cursor_bg = vim.fn.synIDattr(vim.fn.synIDtrans(vim.fn.hlID(M.state.config.hl.active_file)), 'bg')

              -- Create temporary highlight group
              local temp_hl_name = 'FFFGitBorderSelected_' .. i
              if border_fg ~= '' and cursor_bg ~= '' then
                vim.api.nvim_set_hl(0, temp_hl_name, { fg = border_fg, bg = cursor_bg })
                border_hl = temp_hl_name
              else
                border_hl = git_utils.get_border_highlight_selected(item.git_status)
              end
            else
              border_hl = M.state.config.hl.active_file
            end
          else
            border_hl = git_utils.get_border_highlight(item.git_status)
          end

          if border_hl and border_hl ~= '' then
            vim.api.nvim_buf_set_extmark(M.state.list_buf, M.state.ns_id, line_idx - 1, 0, {
              sign_text = border_char,
              sign_hl_group = border_hl,
              priority = 1000,
            })
          end
        elseif is_cursor_item then
          vim.api.nvim_buf_set_extmark(M.state.list_buf, M.state.ns_id, line_idx - 1, 0, {
            sign_text = ' ',
            sign_hl_group = M.state.config.hl.active_file,
            priority = 1000,
          })
        end

        if M.state.selected_files[item.path] then
          local selection_hl = is_cursor_item and M.state.config.hl.selected_active or M.state.config.hl.selected

          vim.api.nvim_buf_set_extmark(M.state.list_buf, M.state.ns_id, line_idx - 1, 0, {
            sign_text = '▊',
            sign_hl_group = selection_hl,
            priority = 1001, -- Higher than git status (1000)
          })
        end

        local match_start, match_end = string.find(line_content, M.state.query, 1)
        if match_start and match_end then
          vim.api.nvim_buf_add_highlight(
            M.state.list_buf,
            M.state.ns_id,
            config.hl.matched or 'IncSearch',
            line_idx - 1,
            match_start - 1,
            match_end
          )
        end
      end

      ::continue::
    end

    combo_renderer.render_highlights_and_overlays(
      combo_item_index,
      combo_header_text_len,
      M.state.list_buf,
      M.state.list_win,
      M.state.ns_id,
      M.state.config.hl.border,
      item_to_lines
    )
  end
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
  local location_changed = not vim.deep_equal(M.state.last_preview_location, M.state.location)

  if M.state.last_preview_file == item.path and not location_changed then return end

  preview.clear()

  M.state.last_preview_file = item.path
  M.state.last_preview_location = vim.deepcopy(M.state.location)

  local relative_path = item.relative_path or item.path
  local max_title_width = vim.api.nvim_win_get_width(M.state.preview_win)

  local title
  local target_length = max_title_width

  if #relative_path + 2 <= target_length then
    title = string.format(' %s ', relative_path)
  else
    local available_chars = target_length - 2

    local filename = vim.fn.fnamemodify(relative_path, ':t')
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

        local segments_to_show = { filename }
        local current_length = #filename + 4 -- 4 for '../' prefix and spaces

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
        local truncated_filename = filename:sub(1, available_chars - 3) .. '...'
        title = string.format(' %s ', truncated_filename)
      end
    end
  end

  vim.api.nvim_win_set_config(M.state.preview_win, {
    title = title,
    title_pos = 'left',
  })

  if M.state.file_info_buf then preview.update_file_info_buffer(item, M.state.file_info_buf, M.state.cursor) end

  preview.set_preview_window(M.state.preview_win)
  preview.preview(item.path, M.state.preview_buf, M.state.location)
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
  local available_width = win_width - 2 -- Account for borders
  local status_len = #status_info

  local col_position = available_width - status_len

  vim.api.nvim_buf_set_extmark(M.state.input_buf, M.state.ns_id, 0, 0, {
    virt_text = { { status_info, 'LineNr' } },
    virt_text_win_col = col_position,
  })
end

function M.move_up()
  if not M.state.active then return end
  if #M.state.filtered_items == 0 then return end

  M.state.cursor = math.max(M.state.cursor - 1, 1)

  M.render_list()
  M.update_preview()
  M.update_status()
end

function M.move_down()
  if not M.state.active then return end
  if #M.state.filtered_items == 0 then return end

  M.state.cursor = math.min(M.state.cursor + 1, #M.state.filtered_items)

  M.render_list()
  M.update_preview()
  M.update_status()
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

  -- Fetch query at current offset from Rust
  local fuzzy = require('fff.core').ensure_initialized()
  local ok, query = pcall(fuzzy.get_historical_query, M.state.history_offset)

  if not ok or not query then
    -- Reached end of history, wrap to beginning
    M.state.history_offset = 0
    ok, query = pcall(fuzzy.get_historical_query, 0)

    if not ok or not query then
      -- No history available at all
      vim.notify('No query history available', vim.log.levels.INFO)
      M.state.history_offset = nil
      return
    end
  end

  M.state.next_search_force_combo_boost = true

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

--- Toggle selection for the current item
function M.toggle_select()
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then return end

  local item = items[M.state.cursor]
  if not item or not item.path then return end

  -- Toggle selection using Set pattern:
  -- Selected: { [path] = true } - key exists in table
  -- Deselected: key removed from table via nil (NOT set to false)
  -- This is important: setting to nil removes the key, so pairs() won't iterate over it
  local was_selected = M.state.selected_files[item.path]
  if was_selected then
    M.state.selected_files[item.path] = nil -- Remove from set
  else
    M.state.selected_files[item.path] = true -- Add to set
  end

  M.render_list()

  -- only when selecting the element not deslecting
  if not was_selected then
    if get_prompt_position() == 'bottom' then
      M.move_up()
    else
      M.move_down()
    end
  end
end

--- Send selected files to quickfix list and close picker
function M.send_to_quickfix()
  if not M.state.active then return end

  -- Collect selected files from the Set-like structure
  -- No need to filter for 'false' values because deselected files are removed from the table (set to nil)
  -- The pairs() iterator only iterates over keys that exist in the table
  -- So only selected files (value = true) will be collected here
  local selected_paths = {}
  for path, _ in pairs(M.state.selected_files) do
    table.insert(selected_paths, path)
  end

  -- If no selections, use current file under cursor
  if #selected_paths == 0 then
    local items = M.state.filtered_items
    if #items > 0 and M.state.cursor <= #items then
      local item = items[M.state.cursor]
      if item and item.path then table.insert(selected_paths, item.path) end
    end
  end

  -- Exit if still nothing to add
  if #selected_paths == 0 then
    vim.notify('No files to send to quickfix', vim.log.levels.WARN)
    return
  end

  -- Close picker first
  M.close()

  -- Build quickfix list entries
  local qf_list = {}
  for _, path in ipairs(selected_paths) do
    table.insert(qf_list, {
      filename = path,
      lnum = 1,
      col = 1,
      text = vim.fn.fnamemodify(path, ':.'), -- Relative path as text
    })
  end

  -- Set quickfix list and open it
  vim.fn.setqflist(qf_list, 'r')
  vim.cmd('copen')

  local count = #selected_paths
  vim.notify(string.format('Added %d file%s to quickfix list', count, count > 1 and 's' or ''), vim.log.levels.INFO)
end

function M.select(action)
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then return end

  local item = items[M.state.cursor]
  if not item then return end

  action = action or 'edit'

  local relative_path = vim.fn.fnamemodify(item.path, ':.')
  local location = M.state.location -- Capture location before closing
  local query = M.state.query -- Capture query before closing for tracking

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
        pcall(fff.track_query_completion, query, item.path)
      end
    end
  end)
end

function M.close()
  if not M.state.active then return end

  vim.cmd('stopinsert')
  M.state.active = false

  combo_renderer.cleanup()

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
  local relative_path =
    vim.fn.fnamemodify(vim.fn.resolve(absolute_path), ':s?' .. vim.fn.escape(base_path, '\\') .. '/??')
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
function M.open(opts)
  if M.state.active then return end

  -- Initialize selection state
  M.state.selected_files = {}

  local merged_config, base_path = initialize_picker(opts)
  if not merged_config then return end

  local current_file_cache = get_current_file_cache(base_path)
  return open_ui_with_state(nil, nil, nil, merged_config, current_file_cache)
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
