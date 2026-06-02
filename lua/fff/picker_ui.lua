local M = {}

local conf = require('fff.conf')
local file_picker = require('fff.file_picker')
local preview = require('fff.file_picker.preview')
local utils = require('fff.utils')
local location_utils = require('fff.location_utils')
local combo_renderer = require('fff.combo_renderer')
local list_separator = require('fff.list_separator')
local list_renderer = require('fff.list_renderer')
local scrollbar = require('fff.scrollbar')
local rust = require('fff.rust')
local layout = require('fff.layout')

local canonicalize_fff_path = utils.canonicalize_fff_path

local preview_config = conf.get().preview
if preview_config then preview.setup(preview_config) end

local function get_prompt_position() return layout.resolve_prompt_position(M.state.config) end

local function suspend_paste()
  if not vim.o.paste then return false end
  vim.o.paste = false
  return true
end

local function restore_paste(should_restore)
  if should_restore then vim.o.paste = true end
end

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
  preview_visible = false, -- True when preview window will be rendered (config + screen size)

  items = {},
  filtered_items = {},
  cursor = 1,
  top = 1,
  query = '',
  line_to_item = {},
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

  config = nil, -- @type FFFConfig|nil

  -- Custom renderer (optional, defaults to file_renderer if not provided)
  renderer = nil,

  ns_id = nil,

  last_status_info = nil,
  restore_paste = false,

  last_preview_file = nil,
  last_preview_location = nil, -- Track last preview location to detect changes

  preview_timer = nil, ---@type uv.uv_timer_t|nil -- Separate timer for preview updates
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

function M.resolve_winhl(kind)
  local hl = M.state.config.hl
  local winhl = hl.winhl
  local default_winhl = string.format('Normal:%s,FloatBorder:%s,FloatTitle:%s', hl.normal, hl.border, hl.title)

  if winhl == nil then return default_winhl end
  if type(winhl) == 'string' then return winhl end
  if type(winhl) == 'table' then return winhl[kind] or default_winhl end
  return default_winhl
end

local function open_preview(win_cfg)
  if not win_cfg then return end
  if M.state.preview_win and vim.api.nvim_win_is_valid(M.state.preview_win) then return end

  if not (M.state.preview_buf and vim.api.nvim_buf_is_valid(M.state.preview_buf)) then
    M.state.preview_buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_set_option_value('bufhidden', 'wipe', { buf = M.state.preview_buf })
    vim.api.nvim_buf_set_name(M.state.preview_buf, 'fffile preview')
    vim.api.nvim_set_option_value('buftype', 'nofile', { buf = M.state.preview_buf })
    vim.api.nvim_set_option_value('filetype', 'fff_preview', { buf = M.state.preview_buf })
    vim.api.nvim_set_option_value('modifiable', false, { buf = M.state.preview_buf })
  end

  M.state.preview_win = vim.api.nvim_open_win(M.state.preview_buf, false, win_cfg)

  local win_hl = M.resolve_winhl('preview')

  vim.api.nvim_set_option_value('wrap', false, { win = M.state.preview_win })
  -- Match line is highlighted via extmark (line_hl_group + number_hl_group)
  -- in location_utils, so the preview window itself keeps cursorline off.
  -- This way paging with <C-d>/<C-u> moves the cursor freely without dragging
  -- the highlight off the actual match line.
  vim.api.nvim_set_option_value('cursorline', false, { win = M.state.preview_win })
  vim.api.nvim_set_option_value('cursorlineopt', vim.o.cursorlineopt, { win = M.state.preview_win })
  vim.api.nvim_set_option_value(
    'number',
    M.state.mode == 'grep' or (preview_config and preview_config.line_numbers or false),
    { win = M.state.preview_win }
  )
  vim.api.nvim_set_option_value('relativenumber', false, { win = M.state.preview_win })
  vim.api.nvim_set_option_value('signcolumn', 'no', { win = M.state.preview_win })
  vim.api.nvim_set_option_value('foldcolumn', '0', { win = M.state.preview_win })
  vim.api.nvim_set_option_value('winhighlight', win_hl, { win = M.state.preview_win })

  preview.set_preview_window(M.state.preview_win)
end

--- Tear down the preview window and buffer. Called when the layout decides
--- the preview no longer fits.
local function close_preview()
  if M.state.preview_win and vim.api.nvim_win_is_valid(M.state.preview_win) then
    vim.api.nvim_win_close(M.state.preview_win, true)
  end
  M.state.preview_win = nil

  if M.state.preview_buf and vim.api.nvim_buf_is_valid(M.state.preview_buf) then
    preview.clear_buffer(M.state.preview_buf)
    vim.api.nvim_buf_delete(M.state.preview_buf, { force = true })
  end
  M.state.preview_buf = nil
  M.state.last_preview_file = nil
  M.state.last_preview_location = nil
end

function M.create_ui()
  local config = M.state.config
  if not config then return false end

  -- Prompt editing should behave consistently even if the user has :set paste.
  M.state.restore_paste = suspend_paste()

  if not M.state.ns_id then
    M.state.ns_id = vim.api.nvim_create_namespace('fff_picker_status')
    list_separator.init(M.state.ns_id)
  end

  local computed_layout = layout.compute(config, conf.preview_enabled(config))
  local win_configs = computed_layout.win_configs
  local debug_enabled_in_preview = computed_layout.debug_enabled

  M.state.layout = computed_layout.layout
  M.state.preview_visible = computed_layout.preview_visible

  M.state.input_buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_set_option_value('bufhidden', 'wipe', { buf = M.state.input_buf })

  M.state.list_buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_set_option_value('bufhidden', 'wipe', { buf = M.state.list_buf })

  if debug_enabled_in_preview then
    M.state.file_info_buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_set_option_value('bufhidden', 'wipe', { buf = M.state.file_info_buf })
  else
    M.state.file_info_buf = nil
  end

  M.state.list_win = vim.api.nvim_open_win(M.state.list_buf, false, win_configs.list)
  if debug_enabled_in_preview and win_configs.file_info then
    M.state.file_info_win = vim.api.nvim_open_win(M.state.file_info_buf, false, win_configs.file_info)
  else
    M.state.file_info_win = nil
  end

  if M.state.preview_visible then open_preview(win_configs.preview) end

  M.state.input_win = vim.api.nvim_open_win(M.state.input_buf, false, win_configs.input)

  M.setup_buffers()
  M.setup_windows()
  M.setup_keymaps()

  vim.api.nvim_set_current_win(M.state.input_win)

  M.update_results_sync()
  M.clear_preview()
  M.update_status()

  return true
end

function M.setup_buffers()
  vim.api.nvim_buf_set_name(M.state.input_buf, 'fffile search')
  vim.api.nvim_buf_set_name(M.state.list_buf, 'fffiles list')

  vim.api.nvim_set_option_value('buftype', 'prompt', { buf = M.state.input_buf })
  vim.api.nvim_set_option_value('filetype', 'fff_input', { buf = M.state.input_buf })

  vim.fn.prompt_setprompt(M.state.input_buf, M.state.config.prompt)

  -- Changing the contents of the input buffer will trigger Neovim to guess the language in order to provide
  -- syntax highlighting. This makes sure that it's always off.
  vim.api.nvim_create_autocmd('Syntax', {
    buffer = M.state.input_buf,
    callback = function() vim.api.nvim_set_option_value('syntax', '', { buf = M.state.input_buf }) end,
  })

  vim.api.nvim_set_option_value('buftype', 'nofile', { buf = M.state.list_buf })
  vim.api.nvim_set_option_value('filetype', 'fff_list', { buf = M.state.list_buf })
  vim.api.nvim_set_option_value('modifiable', false, { buf = M.state.list_buf })

  if M.state.file_info_buf then
    vim.api.nvim_set_option_value('buftype', 'nofile', { buf = M.state.file_info_buf })
    vim.api.nvim_set_option_value('filetype', 'fff_file_info', { buf = M.state.file_info_buf })
    vim.api.nvim_set_option_value('modifiable', false, { buf = M.state.file_info_buf })
  end
end

function M.setup_windows()
  local prompt_win_hl = M.resolve_winhl('prompt')
  local list_win_hl = M.resolve_winhl('list')
  local file_info_win_hl = M.resolve_winhl('file_info')

  vim.api.nvim_set_option_value('wrap', false, { win = M.state.input_win })
  vim.api.nvim_set_option_value('cursorline', false, { win = M.state.input_win })
  vim.api.nvim_set_option_value('number', false, { win = M.state.input_win })
  vim.api.nvim_set_option_value('relativenumber', false, { win = M.state.input_win })
  vim.api.nvim_set_option_value('signcolumn', 'no', { win = M.state.input_win })
  vim.api.nvim_set_option_value('foldcolumn', '0', { win = M.state.input_win })
  vim.api.nvim_set_option_value('winhighlight', prompt_win_hl, { win = M.state.input_win })

  vim.api.nvim_set_option_value('wrap', false, { win = M.state.list_win })
  vim.api.nvim_set_option_value('cursorline', false, { win = M.state.list_win })
  vim.api.nvim_set_option_value('number', false, { win = M.state.list_win })
  vim.api.nvim_set_option_value('relativenumber', false, { win = M.state.list_win })
  vim.api.nvim_set_option_value('signcolumn', 'yes:1', { win = M.state.list_win }) -- Enable signcolumn for git status borders
  vim.api.nvim_set_option_value('foldcolumn', '0', { win = M.state.list_win })
  vim.api.nvim_set_option_value('winhighlight', list_win_hl, { win = M.state.list_win })

  if M.state.file_info_win and vim.api.nvim_win_is_valid(M.state.file_info_win) then
    vim.api.nvim_set_option_value('wrap', false, { win = M.state.file_info_win })
    vim.api.nvim_set_option_value('cursorline', false, { win = M.state.file_info_win })
    vim.api.nvim_set_option_value('number', false, { win = M.state.file_info_win })
    vim.api.nvim_set_option_value('relativenumber', false, { win = M.state.file_info_win })
    vim.api.nvim_set_option_value('signcolumn', 'no', { win = M.state.file_info_win })
    vim.api.nvim_set_option_value('foldcolumn', '0', { win = M.state.file_info_win })
    vim.api.nvim_set_option_value('winhighlight', file_info_win_hl, { win = M.state.file_info_win })
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

  vim.api.nvim_create_autocmd('VimResized', {
    group = picker_group,
    callback = function()
      if not M.state.active then return end
      vim.schedule(function()
        if not M.state.active then return end
        M.relayout()
      end)
    end,
    desc = 'Re-layout picker on terminal resize',
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

local function handle_mouse_click_or_fallback(action, fallback)
  local pos = vim.fn.getmousepos()

  if M.state.active and pos.winid == M.state.list_win then
    local item_idx = M.state.line_to_item[pos.line]
    if not item_idx then return '' end

    vim.schedule(function()
      if not M.state.active then return end
      if not M.state.filtered_items[item_idx] then return end

      if M.state.cursor ~= item_idx then
        M.state.cursor = item_idx
        M.render_list()
        if M.state.mode == 'grep' or M.state.suggestion_source == 'grep' then
          M.update_preview_smart()
        else
          M.update_preview()
        end
        M.update_status()
      end

      if action then M.select(action) end
    end)
    return ''
  end

  return fallback
end

local function move_list_cursor(direction)
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 then return end

  local wrap_around = M.state.config and M.state.config.wrap_around or false
  local new_cursor = M.state.cursor + direction

  if wrap_around then
    if new_cursor < 1 then
      new_cursor = #items
    elseif new_cursor > #items then
      new_cursor = 1
    end
  else
    new_cursor = math.max(1, math.min(new_cursor, #items))
  end

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
  set_keymap('i', keymaps.cycle_forward_query, M.cycle_forward_query, input_opts)
  set_keymap('n', 'j', M.move_down, input_opts)
  set_keymap('n', 'k', M.move_up, input_opts)
  set_keymap('n', 'q', M.close, input_opts)
  set_keymap('n', keymaps.focus_list, M.focus_list_win, input_opts)
  set_keymap('n', keymaps.focus_preview, M.focus_preview_win, input_opts)

  if M.state.config.prompt_vim_mode then
    set_keymap('n', keymaps.close, M.close, input_opts)
    set_keymap('i', '<C-c>', M.close, input_opts)
  else
    set_keymap({ 'i', 'n' }, keymaps.close, M.close, input_opts)
  end

  set_keymap({ 'i', 'n' }, keymaps.select, M.select, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.select_split, function() M.select('split') end, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.select_vsplit, function() M.select('vsplit') end, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.select_tab, function() M.select('tab') end, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.preview_scroll_up, M.scroll_preview_up, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.preview_scroll_down, M.scroll_preview_down, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.toggle_debug, M.toggle_debug, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.toggle_select, M.toggle_select, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.send_to_quickfix, M.send_to_quickfix, input_opts)
  set_keymap({ 'i', 'n' }, keymaps.cycle_grep_modes, M.cycle_grep_modes, input_opts)

  local input_mouse_opts = vim.tbl_extend('force', input_opts, { expr = true, replace_keycodes = true })
  set_keymap(
    { 'i', 'n' },
    '<LeftMouse>',
    function() return handle_mouse_click_or_fallback(nil, '<LeftMouse>') end,
    input_mouse_opts
  )
  set_keymap(
    { 'i', 'n' },
    '<2-LeftMouse>',
    function() return handle_mouse_click_or_fallback('edit', '<2-LeftMouse>') end,
    input_mouse_opts
  )

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

  local list_mouse_opts = vim.tbl_extend('force', list_opts, { expr = true, replace_keycodes = true })
  set_keymap(
    'n',
    '<LeftMouse>',
    function() return handle_mouse_click_or_fallback(nil, '<LeftMouse>') end,
    list_mouse_opts
  )
  set_keymap(
    'n',
    '<2-LeftMouse>',
    function() return handle_mouse_click_or_fallback('edit', '<2-LeftMouse>') end,
    list_mouse_opts
  )

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

  if M.state.config.prompt_vim_mode then
    vim.api.nvim_create_autocmd({ 'CursorMoved', 'CursorMovedI' }, {
      buffer = M.state.input_buf,
      callback = function()
        local prompt_len = #M.state.config.prompt
        if vim.fn.col('.') <= prompt_len then vim.fn.cursor(vim.fn.line('.'), prompt_len + 1) end
      end,
    })
  end
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
function M.cycle_grep_modes()
  if not M.state.active or M.state.mode ~= 'grep' then return end

  local config = conf.get()
  -- Use grep_config.modes if provided, otherwise fall back to global config
  ---@diagnostic disable-next-line: undefined-field
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

    vim.api.nvim_set_option_value('modifiable', true, { buf = M.state.input_buf })
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
      local grep_result = grep.search(M.state.query, 0, page_size, M.state.grep_config, 'plain')
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
  local ok, err, results
  local page_size = M.state.pagination.page_size

  if page_size == 0 then return false end
  if M.state.mode ~= 'grep' then
    local total = M.state.pagination.total_matched
    if total == 0 then return false end

    -- Calculate max page index
    local max_page_index = math.max(0, math.ceil(total / page_size) - 1)

    -- Clamp page_index to valid range
    new_page_index = math.max(0, math.min(new_page_index, max_page_index))
  end

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
    vim.notify('FFF ERROR: Paginated search failed: ' .. tostring(results))
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
    local cursor_ok, cursor_err = pcall(adjust_cursor_fn, #results)
    if not cursor_ok then
      vim.notify('Error in cursor adjustment: ' .. tostring(cursor_err), vim.log.levels.ERROR)
      return false
    end
  end

  ok, err = pcall(M.render_list)
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
    return M.load_page_at_index(new_page_index, function() M.state.cursor = 1 end)
  end

  local total = M.state.pagination.total_matched
  if total == 0 then return false end

  local max_page_index = math.max(0, math.ceil(total / page_size) - 1)
  if current_page >= max_page_index then return false end

  local new_page_index = current_page + 1

  return M.load_page_at_index(new_page_index, function() M.state.cursor = 1 end)
end

--- Load previous page (scroll up reached beginning)
function M.load_previous_page()
  if M.state.pagination.page_index == 0 then return false end

  local new_page_index = M.state.pagination.page_index - 1

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
  M.state.preview_timer = vim.uv.new_timer()
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
  if not M.state.preview_visible then return end
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then
    M.update_preview()
    return
  end

  ---@diagnostic disable-next-line: need-check-nil
  local item = items[M.state.cursor]
  if not item then
    M.update_preview()
    return
  end

  -- Same file: update immediately (just scrolling/re-highlighting, no file I/O)
  if M.state.last_preview_file == item.relative_path then
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
  -- vim.json.decode may return Blobs for strings with NUL bytes; coerce to string.
  local filename = item.name
  if type(filename) ~= 'string' then filename = filename and tostring(filename) or '' end
  local dir_path = item.directory or ''
  if type(dir_path) ~= 'string' then dir_path = dir_path and tostring(dir_path) or '' end

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

--- Adjust scroll for bottom prompt to eliminate gaps.
--- When the cursor has moved above the bottom viewport (e.g. user scrolled up
--- through many results), follow the cursor instead of forcing the view to the
--- bottom — otherwise the selected item disappears off the top of the window.
local function scroll_to_bottom()
  if not M.state.list_win or not vim.api.nvim_win_is_valid(M.state.list_win) then return end

  local win_height = vim.api.nvim_win_get_height(M.state.list_win)
  local buf_lines = vim.api.nvim_buf_line_count(M.state.list_buf)

  vim.api.nvim_win_call(M.state.list_win, function()
    local view = vim.fn.winsaveview()
    local bottom_topline = math.max(1, buf_lines - win_height + 1)
    local cursor_line = vim.api.nvim_win_get_cursor(M.state.list_win)[1]

    if cursor_line >= bottom_topline then
      -- Cursor is visible when anchored to bottom — keep content near prompt
      view.topline = bottom_topline
    elseif cursor_line < view.topline then
      -- Cursor scrolled above the current viewport — shift topline up just
      -- enough to keep the cursor visible (1 line margin above)
      view.topline = math.max(1, cursor_line - 1)
    elseif cursor_line >= view.topline + win_height then
      -- Cursor below viewport (shouldn't happen often) — snap to bottom
      view.topline = bottom_topline
    end
    -- Otherwise cursor is already within the current viewport — don't move it
    vim.fn.winrestview(view)
  end)
end

--- Render the grep empty state: tips + bordered section of recent files.
--- Called when grep mode has an empty query and no items.
local function render_grep_empty_state(ctx)
  list_separator.hide()

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

  vim.api.nvim_set_option_value('modifiable', true, { buf = M.state.list_buf })
  vim.api.nvim_buf_set_lines(M.state.list_buf, 0, -1, false, content)
  vim.api.nvim_set_option_value('modifiable', false, { buf = M.state.list_buf })

  vim.api.nvim_buf_clear_namespace(M.state.list_buf, M.state.ns_id, 0, -1)

  -- For bottom prompt, ensure empty state is anchored at the bottom
  if prompt_position == 'bottom' then scroll_to_bottom() end
  for _, h in ipairs(hl_cmds) do
    pcall(
      vim.api.nvim_buf_set_extmark,
      M.state.list_buf,
      M.state.ns_id,
      h.row,
      h.col_start,
      { end_col = h.col_end, hl_group = h.hl }
    )
  end

  for i = 0, #content - 1 do
    local line = content[i + 1]
    if
      line and (line:match('^%s+Start typing') or line:match('^%s+Tips') or line:match('^%s+"') or line:match('^%s+!'))
    then
      pcall(
        vim.api.nvim_buf_set_extmark,
        M.state.list_buf,
        M.state.ns_id,
        i,
        0,
        { end_row = i + 1, end_col = 0, hl_group = 'Comment' }
      )
    end
    -- Dim border characters
    if line and (line:match('^%s+[╭╰│]') or line:match('[╮╯│]%s*$')) then
      pcall(
        vim.api.nvim_buf_set_extmark,
        M.state.list_buf,
        M.state.ns_id,
        i,
        0,
        { end_row = i + 1, end_col = 0, hl_group = config.hl.border or 'FloatBorder' }
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

  local win_info = vim.fn.getwininfo(M.state.list_win)[1]
  local text_offset = win_info and win_info.textoff or 2
  local text_width = win_width - text_offset

  -- Combo detection (only in file picker mode with real results, not grep or suggestions)
  local combo_boost_score_multiplier = config.history and config.history.combo_boost_score_multiplier or 100
  local separator = nil
  local combo_info = nil
  if M.state.mode ~= 'grep' and not M.state.suggestion_source then
    combo_info = combo_renderer.detect(
      items,
      file_picker,
      combo_boost_score_multiplier,
      M.state.next_search_force_combo_boost or config.history.min_combo_count == 0
    )
  end
  M.state.next_search_force_combo_boost = false

  if combo_info and M.state.combo_visible then
    -- Text is finalised in finalize_render once we know whether the
    -- separator landed visually above or below the match (depends on iter
    -- direction + which end of the iter the anchor sits on).
    separator = {
      idx = combo_info.idx,
      text = combo_info.text,
      text_hl = config.hl.combo_header,
      border_hl = config.hl.border,
    }
    -- Track the actual anchor so the navigation hide-rule measures distance
    -- from the real separator, not an assumed item-1 anchor.
    M.state.combo_initial_cursor = combo_info.idx
  end

  -- Determine iteration order
  local display_start = 1
  local display_end = #items

  -- Reserve one row for the separator gap when present. Without this, items
  -- fill the list to the brim, the gap overflows the window, and the
  -- separator float (positioned by buffer line) gets clipped or drawn on
  -- top of the input border. Drop the item at the far end of the items
  -- list, opposite the anchor — that's the row farthest from the prompt.
  if separator and (display_end - display_start + 1) >= win_height then
    if separator.idx == display_start then
      display_end = display_end - 1
    else
      display_start = display_start + 1
    end
  end

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
    separator = separator,
    combo_info = combo_info,
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

local function finalize_render(separator_line, ctx)
  if ctx.separator and separator_line then
    -- Arrow points toward the prompt: matches sit between the separator and
    -- the prompt, so the arrow visually leads the eye from "rest of list"
    -- through the separator toward the match + prompt.
    local arrow = ctx.prompt_position == 'bottom' and '↓' or '↑'

    local list_cfg = vim.api.nvim_win_get_config(M.state.list_win)
    -- list_cfg.row is on the top border; content starts at list_cfg.row + 1.
    -- separator_line is 1-based -> add directly.
    local screen_row = list_cfg.row + separator_line

    list_separator.update({
      list_win = M.state.list_win,
      row = screen_row,
      text = arrow .. ' ' .. ctx.separator.text,
      text_hl = ctx.separator.text_hl,
      border_hl = ctx.separator.border_hl,
    })
  else
    list_separator.hide()
  end

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
  if M.state.mode == 'grep' and #ctx.items == 0 then
    M.state.line_to_item = {}
    render_grep_empty_state(ctx)
    return
  end

  local item_to_lines, separator_line = list_renderer.render(ctx, M.state.list_buf, M.state.list_win, M.state.ns_id)

  local line_to_item = {}
  for item_idx, mapping in pairs(item_to_lines) do
    for line = mapping.first, mapping.last do
      line_to_item[line] = item_idx
    end
  end
  M.state.line_to_item = line_to_item

  -- For bottom prompt, always ensure content is anchored at the bottom after rendering
  -- This prevents results from appearing in the middle when there are few items
  if ctx.prompt_position == 'bottom' then scroll_to_bottom() end

  finalize_render(separator_line, ctx)
end

--- Build and set the preview window title for a given item and location.
--- For grep mode, appends :line to the path.
---@param item table The current item
---@param location table|nil The effective location
function M.update_preview_title(item, location)
  if not M.state.preview_win or not vim.api.nvim_win_is_valid(M.state.preview_win) then return end

  local relative_path = item.relative_path
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
  if not M.state.preview_visible then return end
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then
    M.clear_preview()
    M.state.last_preview_file = nil
    M.state.last_preview_location = nil
    return
  end

  ---@diagnostic disable-next-line: need-check-nil
  local item = items[M.state.cursor]
  if not item then
    M.clear_preview()
    M.state.last_preview_file = nil
    M.state.last_preview_location = nil
    return
  end

  -- Check if we need to update the preview (file changed OR location changed)
  local effective_location = M.state.location

  -- Fallback: if location is nil but query has a :line suffix, parse it directly
  if not effective_location and M.state.query and M.state.query ~= '' then
    local line_str = M.state.query:match(':(%d+)$')
    if line_str then
      local line_num = tonumber(line_str)
      if line_num and line_num > 0 then
        local l, c = M.state.query:match(':(%d+):(%d+)$')
        if l then
          effective_location = { line = tonumber(l), col = tonumber(c) }
        else
          effective_location = { line = line_num }
        end
      end
    end
  end

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

  if M.state.last_preview_file == item.relative_path and not location_changed then return end

  -- Same file, different location: just scroll and re-highlight instead of reloading
  if M.state.last_preview_file == item.relative_path and location_changed then
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

  M.state.last_preview_file = item.relative_path
  M.state.last_preview_location = effective_location and vim.deepcopy(effective_location) or nil

  M.update_preview_title(item, effective_location)

  if M.state.file_info_buf then
    preview.update_file_info_buffer(item, M.state.file_info_buf, M.state.cursor, M.state.preview_win)
    if M.state.file_info_win and vim.api.nvim_win_is_valid(M.state.file_info_win) then
      local rel = item.relative_path or item.path or ''
      pcall(vim.api.nvim_win_set_config, M.state.file_info_win, { title = ' ' .. rel .. ' ', title_pos = 'left' })
    end
  end

  preview.set_preview_window(M.state.preview_win)
  preview.preview(canonicalize_fff_path(item.relative_path), M.state.preview_buf, effective_location, item.is_binary)
end

--- Clear preview
function M.clear_preview()
  if not M.state.active then return end
  if not M.state.preview_visible then return end

  vim.api.nvim_win_set_config(M.state.preview_win, {
    title = ' Preview ',
    title_pos = 'left',
  })

  if M.state.file_info_buf then
    vim.api.nvim_set_option_value('modifiable', true, { buf = M.state.file_info_buf })
    vim.api.nvim_buf_set_lines(M.state.file_info_buf, 0, -1, false, {})
    vim.api.nvim_set_option_value('modifiable', false, { buf = M.state.file_info_buf })
    pcall(vim.api.nvim_buf_clear_namespace, M.state.file_info_buf, preview.file_info_ns, 0, -1)
  end

  vim.api.nvim_set_option_value('modifiable', true, { buf = M.state.preview_buf })
  vim.api.nvim_buf_set_lines(M.state.preview_buf, 0, -1, false, { 'No preview available' })
  vim.api.nvim_set_option_value('modifiable', false, { buf = M.state.preview_buf })
end

--- Update status information on the right side of input using virtual text
function M.update_status(progress)
  if not M.state.active or not M.state.ns_id then return end
  local config = M.state.config
  if config == nil then return end

  if M.state.mode == 'grep' then
    -- Determine available modes to decide if we should show the mode indicator
    -- Use grep_config.modes if provided, otherwise fall back to global config
    ---@diagnostic disable-next-line: undefined-field
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

    local keybind = config.keymaps.cycle_grep_modes
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
      hl = config.hl.grep_plain_active or 'Comment'
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

    if fallback_label then
      local total_len = #fallback_label
      local col_position = available_width - total_len
      vim.api.nvim_buf_set_extmark(M.state.input_buf, M.state.ns_id, 0, 0, {
        virt_text = { { fallback_label, 'DiagnosticWarn' } },
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

--- Wrap cursor to the first page, first item (best result)
function M.wrap_to_first()
  if M.state.pagination.page_index == 0 then
    -- Already on first page, just move cursor
    M.state.cursor = 1
    return true
  end

  -- For non-grep mode, jump directly to page 0
  if M.state.mode ~= 'grep' then
    return M.load_page_at_index(0, function() M.state.cursor = 1 end)
  end

  -- For grep mode, we can only go back if page 0 offset is recorded
  if M.state.pagination.grep_file_offsets[1] ~= nil then
    return M.load_page_at_index(0, function() M.state.cursor = 1 end)
  end

  M.state.cursor = 1
  return true
end

--- Wrap cursor to the last page, last item (worst result)
function M.wrap_to_last()
  local page_size = M.state.pagination.page_size
  if page_size == 0 then return false end

  if M.state.mode ~= 'grep' then
    local total = M.state.pagination.total_matched
    if total == 0 then return false end
    local max_page_index = math.max(0, math.ceil(total / page_size) - 1)

    if M.state.pagination.page_index == max_page_index then
      -- Already on last page, just move cursor to last item
      M.state.cursor = #M.state.filtered_items
      return true
    end

    return M.load_page_at_index(max_page_index, function(result_count) M.state.cursor = result_count end)
  end

  -- For grep mode, we can't jump to last page (sequential offsets required)
  -- Just wrap within current page
  M.state.cursor = #M.state.filtered_items
  return true
end

--- After cursor moves, decide whether the combo separator should hide.
--- Hide rule: cursor has moved more than 50% of a page *past* the separator
--- anchor (in the direction away from it). Direction-agnostic w.r.t. prompt
--- position because we measure against the item index of the anchor, not its
--- visual row.
local function maybe_hide_combo_separator()
  if not (M.state.combo_initial_cursor and M.state.combo_visible) then return end
  local distance_past = M.state.cursor - M.state.combo_initial_cursor
  -- Anchor at index 1 → "past" means cursor index grew. Anchor at #items
  -- → "past" means cursor index shrank. Use absolute distance + a sign that
  -- matches which side of the anchor the user crossed to.
  if distance_past == 0 then return end
  local half_page = math.floor(M.state.pagination.page_size * 0.5)
  if math.abs(distance_past) <= half_page then return end

  M.state.combo_visible = false
  list_separator.hide()
  M.render_list()
  if get_prompt_position() == 'bottom' then scroll_to_bottom() end
end

function M.move_up()
  if not M.state.active then return end
  if #M.state.filtered_items == 0 then return end

  local prompt_position = get_prompt_position()
  local items_count = #M.state.filtered_items
  local wrap_around = M.state.config and M.state.config.wrap_around or false

  -- Pagination logic depends on prompt position
  if prompt_position == 'bottom' then
    -- Bottom prompt with reverse rendering: visually moving UP means cursor INCREASES
    -- because higher index items are rendered at lower line numbers
    local near_bottom = M.state.cursor >= (items_count - M.state.pagination.prefetch_margin)
    local at_last_item = M.state.cursor >= items_count

    if near_bottom and at_last_item then
      local page_size = M.state.pagination.page_size
      local has_more = false
      if page_size > 0 then
        if M.state.mode == 'grep' then
          has_more = M.state.pagination.grep_next_file_offset > 0
        else
          local max_page = math.max(0, math.ceil(M.state.pagination.total_matched / page_size) - 1)
          has_more = M.state.pagination.page_index < max_page
        end
      end

      if has_more then
        -- More pages available: paginate normally
        M.load_next_page()
        return
      elseif wrap_around then
        -- At global end (last item on last page): wrap to first
        M.wrap_to_first()
      end
    else
      M.state.cursor = math.min(M.state.cursor + 1, items_count)
    end
  else
    -- Top prompt: scrolling UP means going to BETTER results (previous page)
    if M.state.cursor <= M.state.pagination.prefetch_margin + 1 and M.state.cursor <= 1 then
      if M.state.pagination.page_index > 0 then
        -- More pages available: paginate normally
        vim.schedule(M.load_previous_page)
        return
      elseif wrap_around then
        -- At global start (first item on first page): wrap to last
        M.wrap_to_last()
      end
    else
      M.state.cursor = math.max(M.state.cursor - 1, 1)
    end
  end

  M.render_list()
  if M.state.mode == 'grep' or M.state.suggestion_source == 'grep' then
    M.update_preview_smart()
  else
    M.update_preview()
  end
  M.update_status()

  maybe_hide_combo_separator()
end

function M.move_down()
  if not M.state.active then return end
  if #M.state.filtered_items == 0 then return end

  local prompt_position = get_prompt_position()
  local items_count = #M.state.filtered_items
  local wrap_around = M.state.config and M.state.config.wrap_around or false

  -- Pagination logic depends on prompt position
  if prompt_position == 'bottom' then
    -- Bottom prompt with reverse rendering: visually moving DOWN means cursor DECREASES
    -- because lower index items (better) are rendered at higher line numbers
    if M.state.cursor <= M.state.pagination.prefetch_margin + 1 and M.state.cursor <= 1 then
      if M.state.pagination.page_index > 0 then
        -- More pages available: paginate normally
        vim.schedule(M.load_previous_page)
        return
      elseif wrap_around then
        -- At global start (first item on first page): wrap to last
        M.wrap_to_last()
      end
    else
      M.state.cursor = math.max(M.state.cursor - 1, 1)
    end
  else
    -- Top prompt: scrolling DOWN means going to WORSE results (next page)
    local near_bottom = M.state.cursor >= (items_count - M.state.pagination.prefetch_margin)
    local at_last_item = M.state.cursor >= items_count

    if near_bottom and at_last_item then
      local page_size = M.state.pagination.page_size
      local has_more = false
      if page_size > 0 then
        if M.state.mode == 'grep' then
          has_more = M.state.pagination.grep_next_file_offset > 0
        else
          local max_page = math.max(0, math.ceil(M.state.pagination.total_matched / page_size) - 1)
          has_more = M.state.pagination.page_index < max_page
        end
      end

      if has_more then
        -- More pages available: paginate normally
        M.load_next_page()
        return
      elseif wrap_around then
        -- At global end (last item on last page): wrap to first
        M.wrap_to_first()
      end
    else
      M.state.cursor = math.min(M.state.cursor + 1, items_count)
    end
  end

  M.render_list()
  if M.state.mode == 'grep' or M.state.suggestion_source == 'grep' then
    M.update_preview_smart()
  else
    M.update_preview()
  end
  M.update_status()

  maybe_hide_combo_separator()
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

--- Cycle forward through query history (toward more recent queries).
--- Complements recall_query_from_history which cycles backward.
--- Allows bidirectional navigation without losing your place.
function M.cycle_forward_query()
  if not M.state.active then return end

  -- Initialize offset on first press (start from most recent, same as backward)
  if M.state.history_offset == nil then
    M.state.history_offset = 0
  elseif M.state.history_offset > 0 then
    -- Decrement offset to move forward toward more recent queries
    M.state.history_offset = M.state.history_offset - 1
  else
    -- At the most recent entry (offset 0), can't go further forward
    return
  end

  -- Fetch query at current offset from Rust (grep and file picker have separate histories)
  local fuzzy = require('fff.core').ensure_initialized()
  local history_fn = M.state.mode == 'grep' and fuzzy.get_historical_grep_query or fuzzy.get_historical_query
  local ok, query = pcall(history_fn, M.state.history_offset)

  if not ok or not query then
    -- Shouldn't happen since we validated the offset, but handle gracefully
    M.state.history_offset = nil
    return
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

--- Check whether the given window has 'winfixbuf' enabled.
--- pcall-guarded so this stays safe on Neovim versions that predate the option.
local window_has_winfixbuf = utils.window_has_winfixbuf

--- Find the first visible window with a normal file buffer, skipping the
--- picker's own floats.
--- @return number|nil Window ID of the first suitable window, or nil if none found
local function find_suitable_window()
  local exclude = {}
  exclude[M.state.input_win or -1] = true
  exclude[M.state.list_win or -1] = true
  exclude[M.state.preview_win or -1] = true
  exclude[M.state.file_info_win or -1] = true
  return utils.find_suitable_window(exclude)
end

--- Build a unique key for a grep match occurrence.
--- Format: "path:line:col" — uniquely identifies one match entry.
---@param item table Grep match item with path, line_number, col
---@return string
local function grep_item_key(item)
  return string.format('%s:%d:%d', item.relative_path, item.line_number or 0, item.col or 0)
end

--- Toggle selection for the current item.
--- In grep mode, selection is per-occurrence (individual match line).
--- In file mode, selection is per-file path.
function M.toggle_select()
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then return end

  ---@diagnostic disable-next-line: need-check-nil
  local item = items[M.state.cursor]
  if not item or not item.relative_path then return end

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
    was_selected = M.state.selected_files[item.relative_path]
    if was_selected then
      M.state.selected_files[item.relative_path] = nil
    else
      M.state.selected_files[item.relative_path] = true
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
        local abs = canonicalize_fff_path(item.relative_path)
        if abs then
          table.insert(qf_list, {
            filename = abs,
            lnum = item.line_number or 1,
            col = (item.col or 0) + 1,
            text = item.line_content or vim.fn.fnamemodify(abs, ':.'),
          })
        end
      end
    else
      -- No selections: run an exhaustive search to get all matches
      local grep = require('fff.grep')
      local exhaustive_config = vim.tbl_extend('force', M.state.grep_config or {}, { max_matches_per_file = 0 })
      local exhaustive = grep.search(M.state.query, 0, 10000, exhaustive_config, M.state.grep_mode)
      local all_items = exhaustive and exhaustive.items or {}

      if #all_items == 0 then
        vim.notify('No matches to send to quickfix', vim.log.levels.WARN)
        return
      end

      for _, item in ipairs(all_items) do
        local abs = canonicalize_fff_path(item.relative_path)
        if abs then
          table.insert(qf_list, {
            filename = abs,
            lnum = item.line_number or 1,
            col = (item.col or 0) + 1,
            text = item.line_content or vim.fn.fnamemodify(abs, ':.'),
          })
        end
      end
    end
  else
    -- Normal file mode: per-file entries at line 1
    local paths = {}

    -- Collect from explicit selections, or fall back to all visible items.
    -- selected_files is keyed by relative_path; filtered_items carries relative_path too.
    if next(M.state.selected_files) then
      for relative_path, _ in pairs(M.state.selected_files) do
        table.insert(paths, canonicalize_fff_path(relative_path))
      end
    else
      for _, item in ipairs(M.state.filtered_items) do
        local abs = canonicalize_fff_path(item.relative_path)
        if abs then table.insert(paths, abs) end
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

  vim.fn.setqflist(qf_list)
  vim.cmd('copen')

  local count = #qf_list
  local unit = is_grep and (count == 1 and 'match' or 'matches') or (count == 1 and 'file' or 'files')
  vim.notify(string.format('Added %d %s to quickfix list', count, unit), vim.log.levels.INFO)
end

function M.select(action)
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then return end

  ---@diagnostic disable-next-line: need-check-nil
  local item = items[M.state.cursor]
  if not item then return end

  action = action or 'edit'

  -- Anchor against the indexer's base_path (may differ from cwd), then rephrase
  -- as cwd-relative for a nicer buffer name when possible. When outside cwd,
  -- fnamemodify(':.') leaves the absolute path intact.
  local abs_path = canonicalize_fff_path(item.relative_path)
  if not abs_path then return end
  local relative_path = vim.fn.fnamemodify(abs_path, ':.')
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

  -- Fallback: if location is nil but query has a :line suffix, parse it directly
  if not location and query and query ~= '' then
    local line_str = query:match(':(%d+)$')
    if line_str then
      local line_num = tonumber(line_str)
      if line_num and line_num > 0 then
        local col_and_line = query:match(':(%d+):(%d+)$')
        if col_and_line then
          local l, c = query:match(':(%d+):(%d+)$')
          location = { line = tonumber(l), col = tonumber(c) }
        else
          location = { line = line_num }
        end
      end
    end
  end

  vim.cmd('stopinsert')
  M.close()

  -- Defer file open past picker float teardown. Without this, foldexpr is not
  -- recomputed on the new window (folds appear missing) on some platforms.
  vim.schedule(function()
    if action == 'edit' then
      local current_win = vim.api.nvim_get_current_win()
      local current_buf = vim.api.nvim_get_current_buf()
      local current_buftype = vim.api.nvim_get_option_value('buftype', { buf = current_buf })
      local current_buf_modifiable = vim.api.nvim_get_option_value('modifiable', { buf = current_buf })
      local current_winfixbuf = window_has_winfixbuf(current_win)

      -- If the current window can't host a new buffer (special buftype, non-modifiable,
      -- or 'winfixbuf' locking it), retarget a suitable window or fall back to a split.
      -- Without this, :edit raises E1513 ("Cannot switch buffer. 'winfixbuf' is enabled")
      -- whenever the picker is invoked from a window pinned via :h winfixbuf.
      local opened_via_split = false
      if current_buftype ~= '' or not current_buf_modifiable or current_winfixbuf then
        local suitable_win = find_suitable_window()
        if suitable_win then
          vim.api.nvim_set_current_win(suitable_win)
        elseif current_winfixbuf then
          vim.cmd('split ' .. vim.fn.fnameescape(relative_path))
          opened_via_split = true
        end
      end

      if not opened_via_split then vim.cmd('edit ' .. vim.fn.fnameescape(relative_path)) end
    elseif action == 'split' then
      vim.cmd('split ' .. vim.fn.fnameescape(relative_path))
    elseif action == 'vsplit' then
      vim.cmd('vsplit ' .. vim.fn.fnameescape(relative_path))
    elseif action == 'tab' then
      vim.cmd('tabedit ' .. vim.fn.fnameescape(relative_path))
    end

    if location then location_utils.jump_to_location(location) end

    if query and query ~= '' then
      local config = conf.get()
      if config.history and config.history.enabled then
        local fff = require('fff.core').ensure_initialized()
        -- Track in background thread (non-blocking, handled by Rust)
        if mode == 'grep' then
          pcall(fff.track_grep_query, query)
        else
          pcall(fff.track_query_completion, query, item.relative_path)
        end
      end
    end
  end)
end

function M.relayout()
  if not M.state.active then return end

  local config = M.state.config
  if not config then return end

  local computed_layout = layout.compute(config, conf.preview_enabled(config))
  local win_configs = computed_layout.win_configs
  M.state.layout = computed_layout.layout
  M.state.preview_visible = computed_layout.preview_visible

  if M.state.list_win and vim.api.nvim_win_is_valid(M.state.list_win) then
    vim.api.nvim_win_set_config(M.state.list_win, win_configs.list)
  end

  if M.state.input_win and vim.api.nvim_win_is_valid(M.state.input_win) then
    vim.api.nvim_win_set_config(M.state.input_win, win_configs.input)
  end

  -- Reconcile preview window with the new visibility decision. When the
  -- terminal shrinks past the threshold we close it; when it grows back we
  -- recreate it.
  local preview_win_alive = M.state.preview_win and vim.api.nvim_win_is_valid(M.state.preview_win)
  if M.state.preview_visible and win_configs.preview then
    if preview_win_alive then
      vim.api.nvim_win_set_config(M.state.preview_win, win_configs.preview)
    else
      open_preview(win_configs.preview)
    end
  elseif preview_win_alive then
    close_preview()
  end

  -- File info panel piggybacks on the preview being side-by-side. Close it
  -- when the new layout drops it; recreate it when the layout brings it back.
  local file_info_win_alive = M.state.file_info_win and vim.api.nvim_win_is_valid(M.state.file_info_win)
  if win_configs.file_info then
    if file_info_win_alive then
      vim.api.nvim_win_set_config(M.state.file_info_win, win_configs.file_info)
    else
      M.state.file_info_buf = vim.api.nvim_create_buf(false, true)
      vim.api.nvim_set_option_value('bufhidden', 'wipe', { buf = M.state.file_info_buf })
      vim.api.nvim_set_option_value('buftype', 'nofile', { buf = M.state.file_info_buf })
      vim.api.nvim_set_option_value('filetype', 'fff_file_info', { buf = M.state.file_info_buf })
      vim.api.nvim_set_option_value('modifiable', false, { buf = M.state.file_info_buf })
      M.state.file_info_win = vim.api.nvim_open_win(M.state.file_info_buf, false, win_configs.file_info)
    end
  elseif file_info_win_alive then
    vim.api.nvim_win_close(M.state.file_info_win, true)
    M.state.file_info_win = nil
    if M.state.file_info_buf and vim.api.nvim_buf_is_valid(M.state.file_info_buf) then
      vim.api.nvim_buf_delete(M.state.file_info_buf, { force = true })
    end
    M.state.file_info_buf = nil
  end

  -- now rerenderw ith the new computed windows
  M.render_list()
  M.update_preview()
  M.update_status()
end

function M.close()
  if not M.state.active then return end

  vim.cmd('stopinsert')
  M.state.active = false

  restore_paste(M.state.restore_paste)

  list_separator.cleanup()
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
  if M.state.preview_buf then buffers[#buffers + 1] = M.state.preview_buf end

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
  M.state.preview_visible = false
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
  M.state.renderer = nil
  M.state.restore_paste = false
  M.state.combo_visible = true
  M.state.combo_initial_cursor = nil
  M.reset_history_state()
  -- Clean up picker focus autocmds
  pcall(vim.api.nvim_del_augroup_by_name, 'fff_picker_focus')
end

--- Helper function to determine current file cache for deprioritization
--- @param base_path string|nil Base path for relative path calculation
--- @return string|nil Current file cache path
local function get_current_file_cache(base_path)
  if not base_path then return nil end
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
--- @return table|nil, string|nil Merged configuration and base path, nil config if initialization failed
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

  -- open_with_callback runs the file-picker flow, never grep. Reset the
  -- renderer/mode/grep_config defensively so we can't inherit stale state
  -- from a previous live_grep session (close() must always do this too,
  -- but belt-and-braces).
  M.state.renderer = nil
  M.state.mode = nil
  M.state.grep_config = nil

  local merged_config, base_path = initialize_picker(opts)
  if not merged_config then return false end

  local current_file_cache = get_current_file_cache(base_path)
  local results = file_picker.search_files(query, current_file_cache, nil, nil, nil)

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
--- @param opts? {cwd?: string, title?: string, prompt?: string, max_results?: number, max_threads?: number, layout?: {width?: number|function, height?: number|function, prompt_position?: string|function, preview_position?: string|function, preview_size?: number|function}, renderer?: table, mode?: string, grep_config?: table, query?: string} Optional configuration to override defaults
function M.open(opts)
  if M.state.active then return end

  M.state.selected_files = {}
  M.state.selected_items = {}
  M.state.renderer = opts and opts.renderer or nil
  M.state.mode = opts and opts.mode or nil
  M.state.grep_config = opts and opts.grep_config or nil

  local merged_config, base_path = initialize_picker(opts)
  if not merged_config then return end

  if base_path then require('fff.core').change_indexing_directory(base_path) end

  -- Initialize grep_mode to first configured mode when opening in grep mode
  if M.state.mode == 'grep' then
    -- Use grep_config.modes if provided, otherwise fall back to global config
    ---@diagnostic disable-next-line: undefined-field
    local modes = (M.state.grep_config and M.state.grep_config.modes)
      or merged_config.grep.modes
      or { 'plain', 'regex', 'fuzzy' }
    M.state.grep_mode = modes[1] or 'plain'
  end

  local current_file_cache = get_current_file_cache(base_path)
  local query = opts and opts.query or nil ---@type string|nil
  return open_ui_with_state(query, nil, nil, merged_config, current_file_cache)
end

function M.monitor_scan_progress(iteration)
  if not M.state.active then return end

  local progress = file_picker.get_scan_progress()

  if progress.is_scanning then
    M.update_status(progress)

    -- progressive decay for larger directories
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

return M
