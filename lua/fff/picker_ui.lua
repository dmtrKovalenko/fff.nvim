local M = {}

local file_picker = require('fff.file_picker')
local preview = require('fff.file_picker.preview')
local icons = require('fff.file_picker.icons')
local git_utils = require('fff.git_utils')
local main = require('fff.main')

if main.config and main.config.preview then preview.setup(main.config.preview) end

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

  config = nil,

  ns_id = nil,

  last_status_info = nil,

  search_timer = nil,
  search_debounce_ms = 50, -- Debounce delay for search

  last_preview_file = nil,
}

--- Create the picker UI
function M.create_ui()
  local config = M.state.config

  if not M.state.ns_id then M.state.ns_id = vim.api.nvim_create_namespace('fff_picker_status') end

  local debug_enabled_in_preview = M.enabled_preview()
    and main.config
    and main.config.debug
    and main.config.debug.show_scores

  local width = math.floor(vim.o.columns * config.width)
  local height = math.floor(vim.o.lines * config.height)
  local col = math.floor((vim.o.columns - width) / 2)
  local row = math.floor((vim.o.lines - height) / 2)

  local preview_width = M.enabled_preview() and math.floor(width * config.preview.width) or 0
  local list_width = width - preview_width - 3 -- Account for separators
  local list_height = height - 4 -- Same as list window height

  local file_info_height = 0
  local preview_height = list_height
  if debug_enabled_in_preview then
    file_info_height = 10 -- Fixed height of 10 lines for file info
    preview_height = list_height - file_info_height -- No subtraction needed - borders are handled by window positioning
  end

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

  M.state.list_win = vim.api.nvim_open_win(M.state.list_buf, false, {
    relative = 'editor',
    width = list_width,
    height = list_height, -- Use calculated list height
    col = col + 1,
    row = row + 1,
    border = 'single',
    style = 'minimal',
    title = ' Files ',
    title_pos = 'left',
  })

  if debug_enabled_in_preview then
    M.state.file_info_win = vim.api.nvim_open_win(M.state.file_info_buf, false, {
      relative = 'editor',
      width = preview_width,
      height = file_info_height,
      col = col + list_width + 3,
      row = row + 1,
      border = 'single',
      style = 'minimal',
      title = ' File Info ',
      title_pos = 'left',
    })
  else
    M.state.file_info_win = nil
  end

  local preview_row = debug_enabled_in_preview and (row + file_info_height + 3) or (row + 1)
  local preview_height_adj = debug_enabled_in_preview and preview_height or (list_height + 2)

  if M.enabled_preview() then
    M.state.preview_win = vim.api.nvim_open_win(M.state.preview_buf, false, {
      relative = 'editor',
      width = preview_width,
      height = preview_height_adj,
      col = col + list_width + 3,
      row = preview_row,
      border = 'single',
      style = 'minimal',
      title = ' PREVIEW TEST TITLE ',
      title_pos = 'left',
    })
  end

  M.state.input_win = vim.api.nvim_open_win(M.state.input_buf, false, {
    relative = 'editor',
    width = list_width,
    height = 1,
    col = col + 1,
    row = row + height - 2,
    border = 'single',
    style = 'minimal',
  })

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

--- Setup buffer options
function M.setup_buffers()
  vim.api.nvim_buf_set_option(M.state.input_buf, 'buftype', 'prompt')
  vim.api.nvim_buf_set_option(M.state.input_buf, 'filetype', 'fff_input')
  vim.fn.prompt_setprompt(M.state.input_buf, M.state.config.prompt)

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

--- Setup window options
function M.setup_windows()
  local hl = M.state.config.hl

  vim.api.nvim_win_set_option(M.state.input_win, 'wrap', false)
  vim.api.nvim_win_set_option(M.state.input_win, 'cursorline', false)
  vim.api.nvim_win_set_option(M.state.input_win, 'number', false)
  vim.api.nvim_win_set_option(M.state.input_win, 'relativenumber', false)
  vim.api.nvim_win_set_option(M.state.input_win, 'signcolumn', 'no')
  vim.api.nvim_win_set_option(M.state.input_win, 'foldcolumn', '0')

  vim.api.nvim_win_set_option(M.state.list_win, 'wrap', false)
  vim.api.nvim_win_set_option(M.state.list_win, 'cursorline', false)
  vim.api.nvim_win_set_option(M.state.list_win, 'number', false)
  vim.api.nvim_win_set_option(M.state.list_win, 'relativenumber', false)
  vim.api.nvim_win_set_option(M.state.list_win, 'signcolumn', 'yes:1') -- Enable signcolumn for git status borders
  vim.api.nvim_win_set_option(M.state.list_win, 'foldcolumn', '0')

  if M.enabled_preview() then
    vim.api.nvim_win_set_option(M.state.preview_win, 'wrap', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'cursorline', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'number', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'relativenumber', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'signcolumn', 'no')
    vim.api.nvim_win_set_option(M.state.preview_win, 'foldcolumn', '0')
  end
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

  local preview_opts = { buffer = M.state.preview_buf, noremap = true, silent = true }

  set_keymap('n', keymaps.close, M.focus_input_win, preview_opts)
  set_keymap('n', keymaps.select, M.select, preview_opts)
  set_keymap('n', keymaps.select_split, function() M.select('split') end, preview_opts)
  set_keymap('n', keymaps.select_vsplit, function() M.select('vsplit') end, preview_opts)
  set_keymap('n', keymaps.select_tab, function() M.select('tab') end, preview_opts)
  set_keymap('n', keymaps.toggle_debug, M.toggle_debug, preview_opts)

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

--- Toggle debug display
function M.toggle_debug()
  local main = require('fff.main')
  local old_debug_state = main.config.debug.show_scores
  main.config.debug.show_scores = not main.config.debug.show_scores
  local status = main.config.debug.show_scores and 'enabled' or 'disabled'
  vim.notify('FFF debug scores ' .. status, vim.log.levels.INFO)

  if old_debug_state ~= main.config.debug.show_scores then
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

  if M.state.search_timer then
    M.state.search_timer:stop()
    M.state.search_timer:close()
    M.state.search_timer = nil
  end

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

  local results = file_picker.search_files(
    M.state.query,
    M.state.config.max_results,
    M.state.config.max_threads,
    M.state.current_file_cache
  )

  -- because the actual files could be different even with same count
  M.state.items = results
  M.state.filtered_items = results
  M.state.cursor = 1
  M.state.top = 1
  M.render_debounced()
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

--- Render the list
function M.render_list()
  if not M.state.active then return end

  local items = M.state.filtered_items
  local lines = {}

  local main = require('fff.main')
  local max_path_width = main.config.ui and main.config.ui.max_path_width or 80
  local debug_enabled = main.config and main.config.debug and main.config.debug.show_scores
  local win_height = vim.api.nvim_win_get_height(M.state.list_win)
  local display_count = math.min(#items, win_height)
  local empty_lines_needed = win_height - display_count

  for i = 1, empty_lines_needed do
    table.insert(lines, '')
  end

  local end_idx = math.min(#items, display_count)
  local items_to_show = {}
  for i = 1, end_idx do
    table.insert(items_to_show, items[i])
  end

  local reversed_items = {}
  for i = #items_to_show, 1, -1 do
    table.insert(reversed_items, items_to_show[i])
  end

  local line_data = {}

  for i, item in ipairs(reversed_items) do
    local icon, icon_hl_group = icons.get_icon_display(item.name, item.extension, false)
    local frecency = ''
    local total_frecency = (item.total_frecency_score or 0)
    local access_frecency = (item.access_frecency_score or 0)
    local mod_frecency = (item.modification_frecency_score or 0)

    if total_frecency > 0 and debug_enabled then
      local indicator = ''
      if mod_frecency >= 6 then -- High modification frecency (recently modified git file)
        indicator = '🔥' -- Fire for recently modified
      elseif access_frecency >= 4 then -- High access frecency (recently accessed)
        indicator = '⭐' -- Star for frequently accessed
      elseif total_frecency >= 3 then -- Medium total frecency
        indicator = '✨' -- Sparkle for moderate activity
      elseif total_frecency >= 1 then -- Low frecency
        indicator = '•' -- Dot for minimal activity
      end
      frecency = string.format(' %s%d', indicator, total_frecency)
    end

    local suffix = frecency
    local current_indicator = ''
    if item.is_current_file then current_indicator = ' (current)' end

    local available_width = math.max(max_path_width - #icon - 1 - #suffix - #current_indicator, 40)
    local filename, dir_path = format_file_display(item, available_width)

    local line
    if dir_path ~= '' then
      line = string.format('%s %s %s%s%s', icon, filename, dir_path, suffix, current_indicator)
    else
      line = string.format('%s %s%s%s', icon, filename, suffix, current_indicator)
    end

    if item.is_current_file then line = string.format('\027[90m%s\027[0m', line) end

    table.insert(lines, line)
    line_data[i] = {
      filename_len = #filename,
      dir_path_len = #dir_path,
      icon_highlight = {
        hl_group = icon_hl_group,
        icon_length = vim.fn.strdisplaywidth(icon),
        git_status = item.git_status,
      },
    }
  end

  local win_width = vim.api.nvim_win_get_width(M.state.list_win)
  local padded_lines = {}
  for _, line in ipairs(lines) do
    local line_len = vim.fn.strdisplaywidth(line)
    local padding = math.max(0, win_width - line_len + 5) -- +5 extra to ensure full coverage
    local padded_line = line .. string.rep(' ', padding)
    table.insert(padded_lines, padded_line)
  end

  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', true)
  vim.api.nvim_buf_set_lines(M.state.list_buf, 0, -1, false, padded_lines)
  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', false)

  if #items > 0 then
    -- cursor=1 means first/best item, which appears at the last line after reversal
    local cursor_line = empty_lines_needed + (display_count - M.state.cursor + 1)

    if cursor_line > 0 and cursor_line <= win_height then
      vim.api.nvim_win_set_cursor(M.state.list_win, { cursor_line, 0 })

      vim.api.nvim_buf_clear_namespace(M.state.list_buf, M.state.ns_id, 0, -1)

      vim.api.nvim_buf_add_highlight(
        M.state.list_buf,
        M.state.ns_id,
        M.state.config.hl.active_file,
        cursor_line - 1,
        0,
        -1
      )

      local current_line = vim.api.nvim_buf_get_lines(M.state.list_buf, cursor_line - 1, cursor_line, false)[1] or ''
      local line_len = vim.fn.strdisplaywidth(current_line)
      local remaining_width = math.max(0, vim.api.nvim_win_get_width(M.state.list_win) - line_len)

      if remaining_width > 0 then
        vim.api.nvim_buf_set_extmark(M.state.list_buf, M.state.ns_id, cursor_line - 1, -1, {
          virt_text = { { string.rep(' ', remaining_width), M.state.config.hl.active_file } },
          virt_text_pos = 'eol',
        })
      end
    end

    for line_idx, line_content in ipairs(lines) do
      if line_content ~= '' then -- Skip empty lines
        local content_line_idx = line_idx - empty_lines_needed

        local icon_info = line_data[content_line_idx].icon_highlight
        if icon_info and icon_info.hl_group and icon_info.icon_length > 0 then
          vim.api.nvim_buf_add_highlight(
            M.state.list_buf,
            M.state.ns_id,
            icon_info.hl_group,
            line_idx - 1,
            0,
            icon_info.icon_length
          )
        end

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

        local debug_start, debug_end = line_content:find('%[%d+|[^%]]*%]')
        if debug_start then
          vim.api.nvim_buf_add_highlight(
            M.state.list_buf,
            M.state.ns_id,
            M.state.config.hl.debug,
            line_idx - 1,
            debug_start - 1,
            debug_end
          )
        end

        local icon_match = line_content:match('^%S+') -- First non-space sequence (icon)
        if icon_match then
          local filename_len = line_data[content_line_idx].filename_len
          local dir_path_len = line_data[content_line_idx].dir_path_len

          if filename_len > 0 and dir_path_len > 0 then
            local prefix_len = #icon_match + 1 + filename_len + 1 -- icon + space + filename + space

            vim.api.nvim_buf_add_highlight(
              M.state.list_buf,
              M.state.ns_id,
              'Comment',
              line_idx - 1,
              prefix_len,
              prefix_len + dir_path_len
            )
          end
        end

        local is_cursor_line = line_idx == cursor_line
        local border_char = ' ' -- render space so it is highlighted
        local border_hl = nil

        if icon_info and icon_info.git_status and git_utils.should_show_border(icon_info.git_status) then
          border_char = git_utils.get_border_char(icon_info.git_status)
          if is_cursor_line then
            border_hl = git_utils.get_border_highlight_selected(icon_info.git_status)
          else
            border_hl = git_utils.get_border_highlight(icon_info.git_status)
          end
        end

        local final_border_hl = border_hl ~= '' and border_hl
          or (is_cursor_line and M.state.config.hl.active_file or '')

        if final_border_hl ~= '' or is_cursor_line then
          vim.api.nvim_buf_set_extmark(M.state.list_buf, M.state.ns_id, line_idx - 1, 0, {
            sign_text = border_char,
            sign_hl_group = final_border_hl ~= '' and final_border_hl or M.state.config.hl.active_file,
            priority = 1000,
          })
        end
      end
    end
  end
end

function M.update_preview()
  if not M.enabled_preview() then return end
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then
    M.clear_preview()
    M.state.last_preview_file = nil
    return
  end

  local item = items[M.state.cursor]
  if not item then
    M.clear_preview()
    M.state.last_preview_file = nil
    return
  end

  if M.state.last_preview_file == item.path then return end

  local preview = require('fff.file_picker.preview')
  preview.clear()

  M.state.last_preview_file = item.path

  local relative_path = item.relative_path or item.path -- Use relative path if available
  local win_width = vim.api.nvim_win_get_width(M.state.preview_win)
  local max_title_width = win_width - 4 -- Account for border and padding

  local title
  if #relative_path <= max_title_width then
    title = string.format(' %s ', relative_path)
  else
    local filename = vim.fn.fnamemodify(relative_path, ':t')
    local dirname = vim.fn.fnamemodify(relative_path, ':h')
    local available_dir_width = max_title_width - #filename - 6 -- Account for '.../' and spaces

    if available_dir_width > 10 then
      local truncated_dir = '...' .. dirname:sub(-available_dir_width + 3)
      title = string.format(' %s/%s ', truncated_dir, filename)
    else
      if #filename > max_title_width - 4 then filename = filename:sub(1, max_title_width - 7) .. '...' end
      title = string.format(' %s ', filename)
    end
  end

  vim.api.nvim_win_set_config(M.state.preview_win, {
    title = title,
    title_pos = 'left',
  })

  if M.state.file_info_buf then preview.update_file_info_buffer(item, M.state.file_info_buf, M.state.cursor) end

  preview.set_preview_window(M.state.preview_win)
  preview.preview(item.path, M.state.preview_buf)
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
    status_info = string.format('%d/%d', search_metadata.total_matched, search_metadata.total_files)
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

--- Move cursor up (towards worse results, which are visually higher)
function M.move_up()
  if not M.state.active then return end

  if M.state.cursor < #M.state.filtered_items then
    M.state.cursor = M.state.cursor + 1
    M.render_list()
    M.update_preview()
    M.update_status()
  end
end

--- Move cursor down (towards better results, which are visually lower)
function M.move_down()
  if not M.state.active then return end

  if M.state.cursor > 1 then
    M.state.cursor = M.state.cursor - 1
    M.render_list()
    M.update_preview()
    M.update_status()
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

function M.select(action)
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then return end

  local item = items[M.state.cursor]
  if not item then return end

  action = action or 'edit'

  local relative_path = vim.fn.fnamemodify(item.path, ':.')
  file_picker.access_file(relative_path)

  vim.cmd('stopinsert')
  M.close()

  if action == 'edit' then
    vim.cmd('edit ' .. vim.fn.fnameescape(relative_path))
  elseif action == 'split' then
    vim.cmd('split ' .. vim.fn.fnameescape(relative_path))
  elseif action == 'vsplit' then
    vim.cmd('vsplit ' .. vim.fn.fnameescape(relative_path))
  elseif action == 'tab' then
    vim.cmd('tabedit ' .. vim.fn.fnameescape(relative_path))
  end
end

function M.close()
  if not M.state.active then return end

  vim.cmd('stopinsert')
  M.state.active = false

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
  M.state.top = 1
  M.state.query = ''
  M.state.ns_id = nil
  M.state.last_preview_file = nil
  M.state.current_file_cache = nil

  if M.state.search_timer then
    M.state.search_timer:stop()
    M.state.search_timer:close()
    M.state.search_timer = nil
  end
end

function M.open(opts)
  if M.state.active then return end

  if not file_picker.is_initialized() then
    local config = {
      base_path = opts and opts.cwd or vim.fn.getcwd(),
      max_results = 100,
      frecency = {
        enabled = true,
        db_path = vim.fn.stdpath('cache') .. '/fff_nvim',
      },
    }

    if not file_picker.setup(config) then
      vim.notify('Failed to initialize file picker', vim.log.levels.ERROR)
      return
    end
  end

  M.state.config = vim.tbl_deep_extend('force', main.config or {}, opts or {})

  if not M.create_ui() then
    vim.notify('Failed to create picker UI', vim.log.levels.ERROR)
    return
  end

  M.state.active = true
  vim.cmd('startinsert!')

  M.monitor_scan_progress(0)
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
