--- FFF.nvim Buffer Picker - Similar to fzf.vim :Buffers command
--- Lists and fuzzy-searches through open buffers with preview

local M = {}

local conf = require('fff.conf')
local preview = require('fff.file_picker.preview')
local icons = require('fff.file_picker.icons')
local utils = require('fff.utils')

--- Buffer access tracking (similar to g:fzf#vim#buffers in fzf.vim)
--- Stores buffer number -> access timestamp
M.buffer_access_times = {}

--- Initialize buffer tracking autocmds
function M.setup_tracking()
  local group = vim.api.nvim_create_augroup('fff_buffer_tracking', { clear = true })

  -- Track buffer access on enter
  vim.api.nvim_create_autocmd({ 'BufWinEnter', 'WinEnter' }, {
    group = group,
    callback = function()
      local bufnr = vim.api.nvim_get_current_buf()
      if vim.fn.buflisted(bufnr) == 1 then
        M.buffer_access_times[bufnr] = vim.uv.hrtime()
      end
    end,
    desc = 'Track buffer access time for FFF buffer picker',
  })

  -- Clean up on buffer delete
  vim.api.nvim_create_autocmd('BufDelete', {
    group = group,
    callback = function(args)
      M.buffer_access_times[args.buf] = nil
    end,
    desc = 'Clean up buffer tracking for FFF',
  })
end

--- Get list of listed buffers, excluding quickfix
--- @return table List of buffer numbers
function M.get_listed_buffers()
  local buffers = {}
  for bufnr = 1, vim.fn.bufnr('$') do
    if vim.fn.buflisted(bufnr) == 1 then
      local buftype = vim.api.nvim_buf_get_option(bufnr, 'buftype')
      if buftype ~= 'quickfix' then
        table.insert(buffers, bufnr)
      end
    end
  end
  return buffers
end

--- Sort buffers by most recently accessed
--- @param buffers table List of buffer numbers
--- @return table Sorted list of buffer numbers
function M.sort_by_access(buffers)
  table.sort(buffers, function(a, b)
    local time_a = M.buffer_access_times[a] or 0
    local time_b = M.buffer_access_times[b] or 0
    return time_a > time_b -- Most recent first
  end)
  return buffers
end

--- Format buffer for display
--- @param bufnr number Buffer number
--- @return table Buffer info table compatible with picker
function M.format_buffer(bufnr)
  local bufinfo = (vim.fn.getbufinfo(bufnr) or {})[1]
  local name = vim.api.nvim_buf_get_name(bufnr)
  local modified = vim.api.nvim_buf_get_option(bufnr, 'modified')
  local readonly = not vim.api.nvim_buf_get_option(bufnr, 'modifiable')
  local current = bufnr == vim.api.nvim_get_current_buf()
  local alternate = bufnr == vim.fn.bufnr('#')

  -- Get relative path or [No Name]
  local display_name
  local relative_path
  if name == '' then
    display_name = '[No Name]'
    relative_path = '[No Name]'
  else
    display_name = vim.fn.fnamemodify(name, ':~:.')
    relative_path = display_name
  end

  -- Get file extension for icon
  local extension = vim.fn.fnamemodify(name, ':e')
  local filename = vim.fn.fnamemodify(name, ':t')
  if filename == '' then
    filename = '[No Name]'
  end

  -- Get line number (cursor position in buffer)
  local line = bufinfo and bufinfo.lnum or 0

  -- Build status indicators
  local status = ''
  if current then
    status = '%'
  elseif alternate then
    status = '#'
  end

  local flags = ''
  if modified then
    flags = flags .. '[+]'
  end
  if readonly then
    flags = flags .. '[RO]'
  end

  return {
    bufnr = bufnr,
    name = filename,
    path = name,
    relative_path = relative_path,
    display_name = display_name,
    extension = extension,
    line = line,
    modified = modified,
    readonly = readonly,
    current = current,
    alternate = alternate,
    status = status,
    flags = flags,
    is_dir = false,
    -- For compatibility with file picker
    directory = vim.fn.fnamemodify(name, ':h'),
  }
end

--- Get all buffers formatted for the picker
--- @return table List of formatted buffer items
function M.get_buffer_items()
  local buffers = M.get_listed_buffers()
  buffers = M.sort_by_access(buffers)

  local items = {}
  for _, bufnr in ipairs(buffers) do
    table.insert(items, M.format_buffer(bufnr))
  end

  return items
end

--- Filter buffers by query (simple fuzzy match)
--- @param items table List of buffer items
--- @param query string Search query
--- @return table Filtered list of buffer items
function M.filter_buffers(items, query)
  if not query or query == '' then
    return items
  end

  local filtered = {}
  local query_lower = query:lower()

  for _, item in ipairs(items) do
    local match_target = (item.display_name or ''):lower()
    if match_target:find(query_lower, 1, true) then
      table.insert(filtered, item)
    end
  end

  return filtered
end

-- ============================================================================
-- Buffer Picker UI (reuses picker_ui patterns)
-- ============================================================================

M.state = {
  active = false,
  input_win = nil,
  input_buf = nil,
  list_win = nil,
  list_buf = nil,
  preview_win = nil,
  preview_buf = nil,
  items = {},
  filtered_items = {},
  cursor = 1,
  query = '',
  config = nil,
  ns_id = nil,
  last_preview_file = nil,
}

local function get_prompt_position()
  local config = M.state.config
  if config and config.layout and config.layout.prompt_position then
    return config.layout.prompt_position
  end
  return 'bottom'
end

function M.is_preview_enabled()
  local preview_state = nil
  if M.state.config and M.state.config.preview then
    preview_state = M.state.config.preview
  end
  if not preview_state then
    return true
  end
  return preview_state.enabled
end

function M.create_ui()
  local config = M.state.config

  if not M.state.ns_id then
    M.state.ns_id = vim.api.nvim_create_namespace('fff_buffer_picker')
  end

  local terminal_width = vim.o.columns
  local terminal_height = vim.o.lines

  -- Calculate dimensions
  local width_ratio = config.layout.width or 0.8
  local height_ratio = config.layout.height or 0.8
  if type(width_ratio) == 'function' then
    width_ratio = width_ratio(terminal_width, terminal_height)
  end
  if type(height_ratio) == 'function' then
    height_ratio = height_ratio(terminal_width, terminal_height)
  end

  local width = math.floor(terminal_width * width_ratio)
  local height = math.floor(terminal_height * height_ratio)
  local col = math.floor((terminal_width - width) / 2)
  local row = math.floor((terminal_height - height) / 2)

  local prompt_position = get_prompt_position()

  -- Calculate preview size
  local preview_size_ratio = config.layout.preview_size or 0.5
  if type(preview_size_ratio) == 'function' then
    preview_size_ratio = preview_size_ratio(terminal_width, terminal_height)
  end
  local preview_width = M.is_preview_enabled() and math.floor(width * preview_size_ratio) or 0
  local list_width = width - preview_width - (M.is_preview_enabled() and 3 or 0)

  -- Create buffers
  M.state.input_buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_option(M.state.input_buf, 'bufhidden', 'wipe')

  M.state.list_buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_option(M.state.list_buf, 'bufhidden', 'wipe')

  if M.is_preview_enabled() then
    M.state.preview_buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_option(M.state.preview_buf, 'bufhidden', 'wipe')
  end

  local list_height = height - 4 -- Account for borders and input

  -- Create list window
  local list_row = prompt_position == 'bottom' and row + 1 or row + 3
  M.state.list_win = vim.api.nvim_open_win(M.state.list_buf, false, {
    relative = 'editor',
    width = list_width,
    height = list_height,
    col = col + 1,
    row = list_row,
    border = prompt_position == 'bottom' and { '┌', '─', '┐', '│', '', '', '', '│' }
      or { '├', '─', '┤', '│', '┘', '─', '└', '│' },
    style = 'minimal',
    title = prompt_position == 'bottom' and ' Buffers ' or nil,
    title_pos = prompt_position == 'bottom' and 'left' or nil,
  })

  -- Create preview window
  if M.is_preview_enabled() then
    M.state.preview_win = vim.api.nvim_open_win(M.state.preview_buf, false, {
      relative = 'editor',
      width = preview_width,
      height = height - 2,
      col = col + list_width + 3,
      row = row + 1,
      border = 'single',
      style = 'minimal',
      title = ' Preview ',
      title_pos = 'left',
    })
  end

  -- Create input window
  local input_row = prompt_position == 'bottom' and row + list_height + 2 or row + 1
  M.state.input_win = vim.api.nvim_open_win(M.state.input_buf, false, {
    relative = 'editor',
    width = list_width,
    height = 1,
    col = col + 1,
    row = input_row,
    border = prompt_position == 'bottom' and { '├', '─', '┤', '│', '┘', '─', '└', '│' }
      or { '┌', '─', '┐', '│', '', '', '', '│' },
    style = 'minimal',
    title = prompt_position == 'top' and ' Buffers ' or nil,
    title_pos = prompt_position == 'top' and 'left' or nil,
  })

  M.setup_buffers()
  M.setup_windows()
  M.setup_keymaps()

  vim.api.nvim_set_current_win(M.state.input_win)

  if M.is_preview_enabled() then
    preview.set_preview_window(M.state.preview_win)
  end

  return true
end

function M.setup_buffers()
  vim.api.nvim_buf_set_name(M.state.input_buf, 'fff buffer search')
  vim.api.nvim_buf_set_name(M.state.list_buf, 'fff buffer list')
  if M.is_preview_enabled() then
    vim.api.nvim_buf_set_name(M.state.preview_buf, 'fff buffer preview')
  end

  vim.api.nvim_buf_set_option(M.state.input_buf, 'buftype', 'prompt')
  vim.api.nvim_buf_set_option(M.state.input_buf, 'filetype', 'fff_buffer_input')
  vim.fn.prompt_setprompt(M.state.input_buf, M.state.config.prompt or '🦆 ')

  vim.api.nvim_buf_set_option(M.state.list_buf, 'buftype', 'nofile')
  vim.api.nvim_buf_set_option(M.state.list_buf, 'filetype', 'fff_buffer_list')
  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', false)

  if M.is_preview_enabled() then
    vim.api.nvim_buf_set_option(M.state.preview_buf, 'buftype', 'nofile')
    vim.api.nvim_buf_set_option(M.state.preview_buf, 'filetype', 'fff_buffer_preview')
    vim.api.nvim_buf_set_option(M.state.preview_buf, 'modifiable', false)
  end
end

function M.setup_windows()
  local hl = M.state.config.hl
  local win_hl = string.format('Normal:%s,FloatBorder:%s,FloatTitle:%s', hl.normal, hl.border, hl.title)

  vim.api.nvim_win_set_option(M.state.input_win, 'wrap', false)
  vim.api.nvim_win_set_option(M.state.input_win, 'cursorline', false)
  vim.api.nvim_win_set_option(M.state.input_win, 'number', false)
  vim.api.nvim_win_set_option(M.state.input_win, 'winhighlight', win_hl)

  vim.api.nvim_win_set_option(M.state.list_win, 'wrap', false)
  vim.api.nvim_win_set_option(M.state.list_win, 'cursorline', false)
  vim.api.nvim_win_set_option(M.state.list_win, 'number', false)
  vim.api.nvim_win_set_option(M.state.list_win, 'signcolumn', 'yes:1')
  vim.api.nvim_win_set_option(M.state.list_win, 'winhighlight', win_hl)

  if M.is_preview_enabled() then
    vim.api.nvim_win_set_option(M.state.preview_win, 'wrap', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'cursorline', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'number', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'winhighlight', win_hl)
  end

  -- Close picker when focus leaves
  local picker_group = vim.api.nvim_create_augroup('fff_buffer_picker_focus', { clear = true })
  local picker_windows = { M.state.input_win, M.state.list_win }
  if M.state.preview_win then
    table.insert(picker_windows, M.state.preview_win)
  end

  vim.api.nvim_create_autocmd('WinLeave', {
    group = picker_group,
    callback = function()
      if not M.state.active then
        return
      end

      local current_win = vim.api.nvim_get_current_win()
      local is_picker_window = vim.tbl_contains(picker_windows, current_win)

      if is_picker_window then
        vim.defer_fn(function()
          if not M.state.active then
            return
          end

          local new_win = vim.api.nvim_get_current_win()
          if not vim.tbl_contains(picker_windows, new_win) then
            M.close()
          end
        end, 10)
      end
    end,
    desc = 'Close buffer picker when focus leaves',
  })
end

function M.setup_keymaps()
  local keymaps = M.state.config.keymaps

  local input_opts = { buffer = M.state.input_buf, noremap = true, silent = true }

  vim.keymap.set('i', keymaps.close, M.close, input_opts)
  vim.keymap.set('i', keymaps.select, M.select, input_opts)
  vim.keymap.set('i', keymaps.select_split, function() M.select('split') end, input_opts)
  vim.keymap.set('i', keymaps.select_vsplit, function() M.select('vsplit') end, input_opts)
  vim.keymap.set('i', keymaps.select_tab, function() M.select('tab') end, input_opts)

  -- Handle both string and table key mappings
  local move_up_keys = type(keymaps.move_up) == 'table' and keymaps.move_up or { keymaps.move_up }
  local move_down_keys = type(keymaps.move_down) == 'table' and keymaps.move_down or { keymaps.move_down }

  for _, key in ipairs(move_up_keys) do
    vim.keymap.set('i', key, M.move_up, input_opts)
  end
  for _, key in ipairs(move_down_keys) do
    vim.keymap.set('i', key, M.move_down, input_opts)
  end

  if keymaps.preview_scroll_up then
    vim.keymap.set('i', keymaps.preview_scroll_up, M.scroll_preview_up, input_opts)
  end
  if keymaps.preview_scroll_down then
    vim.keymap.set('i', keymaps.preview_scroll_down, M.scroll_preview_down, input_opts)
  end

  -- Delete buffer with <C-d>
  vim.keymap.set('i', '<C-d>', M.delete_buffer, input_opts)

  -- Handle input changes
  vim.api.nvim_buf_attach(M.state.input_buf, false, {
    on_lines = function()
      vim.schedule(function()
        M.on_input_change()
      end)
    end,
  })
end

function M.on_input_change()
  if not M.state.active then
    return
  end

  local lines = vim.api.nvim_buf_get_lines(M.state.input_buf, 0, -1, false)
  local prompt_len = #(M.state.config.prompt or '🦆 ')
  local query = ''

  local full_line = lines[1] or ''
  if full_line:sub(1, prompt_len) == (M.state.config.prompt or '🦆 ') then
    query = full_line:sub(prompt_len + 1)
  end

  M.state.query = query
  M.update_results()
end

function M.update_results()
  if not M.state.active then
    return
  end

  -- Get fresh buffer list and filter
  M.state.items = M.get_buffer_items()
  M.state.filtered_items = M.filter_buffers(M.state.items, M.state.query)

  local prompt_position = get_prompt_position()
  if prompt_position == 'bottom' then
    M.state.cursor = #M.state.filtered_items > 0 and #M.state.filtered_items or 1
  else
    M.state.cursor = 1
  end

  M.render_list()
  M.update_preview()
  M.update_status()
end

function M.render_list()
  if not M.state.active then
    return
  end

  local items = M.state.filtered_items
  local win_height = vim.api.nvim_win_get_height(M.state.list_win)
  local win_width = vim.api.nvim_win_get_width(M.state.list_win)
  local display_count = math.min(#items, win_height)
  local prompt_position = get_prompt_position()

  local empty_lines_needed = 0
  local cursor_line = 0

  if #items > 0 then
    if prompt_position == 'bottom' then
      empty_lines_needed = win_height - display_count
      cursor_line = empty_lines_needed + M.state.cursor
    else
      cursor_line = M.state.cursor
    end
    cursor_line = math.max(1, math.min(cursor_line, win_height))
  end

  local lines = {}

  -- Add empty lines for bottom prompt position
  if prompt_position == 'bottom' then
    for _ = 1, empty_lines_needed do
      table.insert(lines, string.rep(' ', win_width))
    end
  end

  -- Format each buffer line
  for i = 1, display_count do
    local item = items[i]
    local icon, icon_hl = icons.get_icon_display(item.name, item.extension, false)

    -- Build the line: [bufnr] status icon name flags path
    local bufnr_str = string.format('[%d]', item.bufnr)
    local status = item.status ~= '' and item.status .. ' ' or '  '
    local flags = item.flags ~= '' and ' ' .. item.flags or ''

    local line = string.format('%s %s%s %s%s  %s', bufnr_str, status, icon, item.name, flags, item.directory or '')

    -- Pad line
    local line_len = vim.fn.strdisplaywidth(line)
    local padding = math.max(0, win_width - line_len + 5)
    table.insert(lines, line .. string.rep(' ', padding))
  end

  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', true)
  vim.api.nvim_buf_set_lines(M.state.list_buf, 0, -1, false, lines)
  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', false)

  -- Clear and set highlights
  vim.api.nvim_buf_clear_namespace(M.state.list_buf, M.state.ns_id, 0, -1)

  if #items > 0 and cursor_line > 0 and cursor_line <= #lines then
    vim.api.nvim_win_set_cursor(M.state.list_win, { cursor_line, 0 })

    -- Highlight cursor line
    vim.api.nvim_buf_add_highlight(
      M.state.list_buf,
      M.state.ns_id,
      M.state.config.hl.active_file,
      cursor_line - 1,
      0,
      -1
    )

    -- Add highlights for each visible item
    for i = 1, display_count do
      local item = items[i]
      local line_idx = empty_lines_needed + i
      local is_cursor_line = line_idx == cursor_line

      -- Highlight buffer number
      local line_content = lines[line_idx] or ''
      local bufnr_end = line_content:find(']') or 0
      if bufnr_end > 0 then
        vim.api.nvim_buf_add_highlight(M.state.list_buf, M.state.ns_id, 'Number', line_idx - 1, 0, bufnr_end)
      end

      -- Highlight status indicator
      if item.current then
        local status_pos = bufnr_end + 1
        vim.api.nvim_buf_add_highlight(M.state.list_buf, M.state.ns_id, 'Conditional', line_idx - 1, status_pos, status_pos + 1)
      elseif item.alternate then
        local status_pos = bufnr_end + 1
        vim.api.nvim_buf_add_highlight(M.state.list_buf, M.state.ns_id, 'Special', line_idx - 1, status_pos, status_pos + 1)
      end

      -- Highlight modified flag
      if item.modified then
        local plus_start = line_content:find('%[%+%]')
        if plus_start then
          vim.api.nvim_buf_add_highlight(M.state.list_buf, M.state.ns_id, 'Exception', line_idx - 1, plus_start - 1, plus_start + 2)
        end
      end

      -- Highlight directory path as comment
      if item.directory and item.directory ~= '' then
        local dir_start = line_content:find(item.directory, 1, true)
        if dir_start then
          vim.api.nvim_buf_add_highlight(M.state.list_buf, M.state.ns_id, 'Comment', line_idx - 1, dir_start - 1, -1)
        end
      end

      -- Sign for current buffer indicator
      if item.current and not is_cursor_line then
        vim.api.nvim_buf_set_extmark(M.state.list_buf, M.state.ns_id, line_idx - 1, 0, {
          sign_text = '▎',
          sign_hl_group = 'Conditional',
          priority = 1000,
        })
      end
    end
  end
end

function M.update_preview()
  if not M.is_preview_enabled() then
    return
  end
  if not M.state.active then
    return
  end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then
    M.clear_preview()
    M.state.last_preview_file = nil
    return
  end

  local item = items[M.state.cursor]
  if not item or item.path == '' then
    M.clear_preview()
    M.state.last_preview_file = nil
    return
  end

  if M.state.last_preview_file == item.path then
    return
  end

  preview.clear()
  M.state.last_preview_file = item.path

  -- Update preview window title
  local title = string.format(' %s ', item.display_name or item.name)
  vim.api.nvim_win_set_config(M.state.preview_win, {
    title = title,
    title_pos = 'left',
  })

  preview.set_preview_window(M.state.preview_win)
  preview.preview(item.path, M.state.preview_buf, { line = item.line })
end

function M.clear_preview()
  if not M.state.active then
    return
  end
  if not M.is_preview_enabled() then
    return
  end

  vim.api.nvim_win_set_config(M.state.preview_win, {
    title = ' Preview ',
    title_pos = 'left',
  })

  vim.api.nvim_buf_set_option(M.state.preview_buf, 'modifiable', true)
  vim.api.nvim_buf_set_lines(M.state.preview_buf, 0, -1, false, { 'No preview available' })
  vim.api.nvim_buf_set_option(M.state.preview_buf, 'modifiable', false)
end

function M.update_status()
  if not M.state.active or not M.state.ns_id then
    return
  end

  local status_info = string.format('%d/%d', #M.state.filtered_items, #M.state.items)

  vim.api.nvim_buf_clear_namespace(M.state.input_buf, M.state.ns_id, 0, -1)

  local win_width = vim.api.nvim_win_get_width(M.state.input_win)
  local col_position = win_width - #status_info - 2

  vim.api.nvim_buf_set_extmark(M.state.input_buf, M.state.ns_id, 0, 0, {
    virt_text = { { status_info, 'LineNr' } },
    virt_text_win_col = col_position,
  })
end

function M.move_up()
  if not M.state.active then
    return
  end
  if #M.state.filtered_items == 0 then
    return
  end

  M.state.cursor = math.max(M.state.cursor - 1, 1)
  M.render_list()
  M.update_preview()
end

function M.move_down()
  if not M.state.active then
    return
  end
  if #M.state.filtered_items == 0 then
    return
  end

  M.state.cursor = math.min(M.state.cursor + 1, #M.state.filtered_items)
  M.render_list()
  M.update_preview()
end

function M.scroll_preview_up()
  if not M.state.active or not M.state.preview_win then
    return
  end
  local win_height = vim.api.nvim_win_get_height(M.state.preview_win)
  preview.scroll(-math.floor(win_height / 2))
end

function M.scroll_preview_down()
  if not M.state.active or not M.state.preview_win then
    return
  end
  local win_height = vim.api.nvim_win_get_height(M.state.preview_win)
  preview.scroll(math.floor(win_height / 2))
end

function M.select(action)
  if not M.state.active then
    return
  end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then
    return
  end

  local item = items[M.state.cursor]
  if not item then
    return
  end

  action = action or 'edit'
  local bufnr = item.bufnr

  vim.cmd('stopinsert')
  M.close()

  if action == 'edit' then
    vim.cmd('buffer ' .. bufnr)
  elseif action == 'split' then
    vim.cmd('sbuffer ' .. bufnr)
  elseif action == 'vsplit' then
    vim.cmd('vertical sbuffer ' .. bufnr)
  elseif action == 'tab' then
    vim.cmd('tab sbuffer ' .. bufnr)
  end

  -- Jump to last cursor position if available
  if item.line and item.line > 0 then
    pcall(vim.api.nvim_win_set_cursor, 0, { item.line, 0 })
  end
end

function M.delete_buffer()
  if not M.state.active then
    return
  end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then
    return
  end

  local item = items[M.state.cursor]
  if not item then
    return
  end

  -- Don't delete if it's the current buffer and it's the only one
  local all_buffers = M.get_listed_buffers()
  if #all_buffers <= 1 then
    vim.notify('Cannot delete the last buffer', vim.log.levels.WARN)
    return
  end

  -- Confirm if buffer is modified
  if item.modified then
    vim.notify('Buffer has unsaved changes. Save first or use :bd!', vim.log.levels.WARN)
    return
  end

  -- Delete the buffer
  pcall(vim.cmd, 'bdelete ' .. item.bufnr)

  -- Update the list
  M.update_results()
end

function M.close()
  if not M.state.active then
    return
  end

  vim.cmd('stopinsert')
  M.state.active = false

  local windows = { M.state.input_win, M.state.list_win, M.state.preview_win }
  for _, win in ipairs(windows) do
    if win and vim.api.nvim_win_is_valid(win) then
      vim.api.nvim_win_close(win, true)
    end
  end

  local buffers = { M.state.input_buf, M.state.list_buf }
  if M.is_preview_enabled() then
    table.insert(buffers, M.state.preview_buf)
  end

  for _, buf in ipairs(buffers) do
    if buf and vim.api.nvim_buf_is_valid(buf) then
      vim.api.nvim_buf_clear_namespace(buf, -1, 0, -1)
      if buf == M.state.preview_buf then
        preview.clear_buffer(buf)
      end
      vim.api.nvim_buf_delete(buf, { force = true })
    end
  end

  -- Reset state
  M.state.input_win = nil
  M.state.list_win = nil
  M.state.preview_win = nil
  M.state.input_buf = nil
  M.state.list_buf = nil
  M.state.preview_buf = nil
  M.state.items = {}
  M.state.filtered_items = {}
  M.state.cursor = 1
  M.state.query = ''
  M.state.ns_id = nil
  M.state.last_preview_file = nil

  pcall(vim.api.nvim_del_augroup_by_name, 'fff_buffer_picker_focus')
end

--- Open the buffer picker
--- @param opts? table Optional configuration to override defaults
function M.open(opts)
  if M.state.active then
    return
  end

  local config = conf.get()
  local merged_config = vim.tbl_deep_extend('force', config or {}, opts or {})

  -- Override title and prompt for buffers
  merged_config.title = opts and opts.title or 'Buffers'
  merged_config.prompt = opts and opts.prompt or '🦆 '

  M.state.config = merged_config
  M.state.active = true

  -- Get buffer items
  M.state.items = M.get_buffer_items()
  M.state.filtered_items = M.state.items

  if not M.create_ui() then
    vim.notify('Failed to create buffer picker UI', vim.log.levels.ERROR)
    M.state.active = false
    return
  end

  -- Set initial cursor position
  local prompt_position = get_prompt_position()
  if prompt_position == 'bottom' then
    M.state.cursor = #M.state.filtered_items > 0 and #M.state.filtered_items or 1
  else
    M.state.cursor = 1
  end

  M.render_list()
  M.update_preview()
  M.update_status()

  vim.cmd('startinsert!')
end

-- Initialize buffer tracking on module load
M.setup_tracking()

return M

