--- FFF.nvim Colors Picker - Similar to fzf.vim :Colors command
--- Lists and fuzzy-searches through colorschemes with live preview

local M = {}

local conf = require('fff.conf')
local utils = require('fff.utils')

--- State for tracking original colorscheme
M.original_colorscheme = nil

--- Get list of available colorschemes from runtime path and packages
--- @return table List of colorscheme names
function M.get_colorschemes()
  local colorschemes = {}
  local seen = {}

  -- Get colorschemes from rtp
  local rtp_colors = vim.fn.globpath(vim.o.runtimepath, 'colors/*.vim', false, true)
  local rtp_lua_colors = vim.fn.globpath(vim.o.runtimepath, 'colors/*.lua', false, true)

  -- Get colorschemes from packpath (packages)
  local pack_colors = vim.fn.globpath(vim.o.packpath, 'pack/*/opt/*/colors/*.vim', false, true)
  local pack_lua_colors = vim.fn.globpath(vim.o.packpath, 'pack/*/opt/*/colors/*.lua', false, true)
  local pack_start_colors = vim.fn.globpath(vim.o.packpath, 'pack/*/start/*/colors/*.vim', false, true)
  local pack_start_lua_colors = vim.fn.globpath(vim.o.packpath, 'pack/*/start/*/colors/*.lua', false, true)

  local all_files = vim.list_extend({}, rtp_colors)
  all_files = vim.list_extend(all_files, rtp_lua_colors)
  all_files = vim.list_extend(all_files, pack_colors)
  all_files = vim.list_extend(all_files, pack_lua_colors)
  all_files = vim.list_extend(all_files, pack_start_colors)
  all_files = vim.list_extend(all_files, pack_start_lua_colors)

  for _, file in ipairs(all_files) do
    local name = vim.fn.fnamemodify(file, ':t:r')
    if name and name ~= '' and not seen[name] then
      seen[name] = true
      table.insert(colorschemes, name)
    end
  end

  -- Sort alphabetically
  table.sort(colorschemes)

  -- Put the current colorscheme at the top if it exists
  if vim.g.colors_name then
    local current = vim.g.colors_name
    -- Remove from current position
    for i, name in ipairs(colorschemes) do
      if name == current then
        table.remove(colorschemes, i)
        break
      end
    end
    -- Insert at the beginning
    table.insert(colorschemes, 1, current)
  end

  return colorschemes
end

--- Format colorscheme for display
--- @param name string Colorscheme name
--- @param idx number Index in the list
--- @return table Colorscheme info table compatible with picker
function M.format_colorscheme(name, idx)
  local is_current = vim.g.colors_name == name

  return {
    name = name,
    path = name, -- For compatibility
    relative_path = name,
    display_name = name,
    current = is_current,
    is_dir = false,
    idx = idx,
  }
end

--- Get all colorschemes formatted for the picker
--- @return table List of formatted colorscheme items
function M.get_colorscheme_items()
  local colorschemes = M.get_colorschemes()
  local items = {}

  for i, name in ipairs(colorschemes) do
    table.insert(items, M.format_colorscheme(name, i))
  end

  return items
end

--- Filter colorschemes by query (simple fuzzy match)
--- @param items table List of colorscheme items
--- @param query string Search query
--- @return table Filtered list of colorscheme items
function M.filter_colorschemes(items, query)
  if not query or query == '' then
    return items
  end

  local filtered = {}
  local query_lower = query:lower()

  for _, item in ipairs(items) do
    local match_target = (item.name or ''):lower()
    if match_target:find(query_lower, 1, true) then
      table.insert(filtered, item)
    end
  end

  return filtered
end

-- ============================================================================
-- Colors Picker UI (reuses picker patterns from buffers.lua)
-- ============================================================================

M.state = {
  active = false,
  input_win = nil,
  input_buf = nil,
  list_win = nil,
  list_buf = nil,
  items = {},
  filtered_items = {},
  cursor = 1,
  query = '',
  config = nil,
  ns_id = nil,
}

local function get_prompt_position()
  local config = M.state.config
  if config and config.layout and config.layout.prompt_position then
    return config.layout.prompt_position
  end
  return 'bottom'
end

function M.create_ui()
  local config = M.state.config

  if not M.state.ns_id then
    M.state.ns_id = vim.api.nvim_create_namespace('fff_colors_picker')
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

  -- For colors picker, use a smaller window that fits content nicely
  local num_items = #M.state.items
  local max_name_width = 0
  for _, item in ipairs(M.state.items) do
    max_name_width = math.max(max_name_width, #item.name)
  end

  -- Size window to fit content (like fzf.vim)
  local list_width = math.min(math.max(max_name_width + 10, 30), width)
  local list_height = math.min(num_items + 2, height - 4)

  local col = math.floor((terminal_width - list_width) / 2)
  local row = math.floor((terminal_height - list_height - 4) / 2)

  local prompt_position = get_prompt_position()

  -- Create buffers
  M.state.input_buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_option(M.state.input_buf, 'bufhidden', 'wipe')

  M.state.list_buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_option(M.state.list_buf, 'bufhidden', 'wipe')

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
    title = prompt_position == 'bottom' and ' Colors ' or nil,
    title_pos = prompt_position == 'bottom' and 'left' or nil,
  })

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
    title = prompt_position == 'top' and ' Colors ' or nil,
    title_pos = prompt_position == 'top' and 'left' or nil,
  })

  M.setup_buffers()
  M.setup_windows()
  M.setup_keymaps()

  vim.api.nvim_set_current_win(M.state.input_win)

  return true
end

function M.setup_buffers()
  vim.api.nvim_buf_set_name(M.state.input_buf, 'fff colors search')
  vim.api.nvim_buf_set_name(M.state.list_buf, 'fff colors list')

  vim.api.nvim_buf_set_option(M.state.input_buf, 'buftype', 'prompt')
  vim.api.nvim_buf_set_option(M.state.input_buf, 'filetype', 'fff_colors_input')
  vim.fn.prompt_setprompt(M.state.input_buf, M.state.config.prompt or 'Colors> ')

  vim.api.nvim_buf_set_option(M.state.list_buf, 'buftype', 'nofile')
  vim.api.nvim_buf_set_option(M.state.list_buf, 'filetype', 'fff_colors_list')
  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', false)
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

  -- Close picker when focus leaves
  local picker_group = vim.api.nvim_create_augroup('fff_colors_picker_focus', { clear = true })
  local picker_windows = { M.state.input_win, M.state.list_win }

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
    desc = 'Close colors picker when focus leaves',
  })
end

function M.setup_keymaps()
  local keymaps = M.state.config.keymaps

  local input_opts = { buffer = M.state.input_buf, noremap = true, silent = true }

  vim.keymap.set('i', keymaps.close, M.close, input_opts)
  vim.keymap.set('i', keymaps.select, M.select, input_opts)

  -- Handle both string and table key mappings
  local move_up_keys = type(keymaps.move_up) == 'table' and keymaps.move_up or { keymaps.move_up }
  local move_down_keys = type(keymaps.move_down) == 'table' and keymaps.move_down or { keymaps.move_down }

  for _, key in ipairs(move_up_keys) do
    vim.keymap.set('i', key, M.move_up, input_opts)
  end
  for _, key in ipairs(move_down_keys) do
    vim.keymap.set('i', key, M.move_down, input_opts)
  end

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
  local prompt_len = #(M.state.config.prompt or 'Colors> ')
  local query = ''

  local full_line = lines[1] or ''
  if full_line:sub(1, prompt_len) == (M.state.config.prompt or 'Colors> ') then
    query = full_line:sub(prompt_len + 1)
  end

  M.state.query = query
  M.update_results()
end

function M.update_results()
  if not M.state.active then
    return
  end

  -- Filter colorschemes
  M.state.filtered_items = M.filter_colorschemes(M.state.items, M.state.query)

  local prompt_position = get_prompt_position()
  if prompt_position == 'bottom' then
    M.state.cursor = #M.state.filtered_items > 0 and #M.state.filtered_items or 1
  else
    M.state.cursor = 1
  end

  M.render_list()
  M.update_status()
  M.preview_colorscheme()
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

  -- Format each colorscheme line
  for i = 1, display_count do
    local item = items[i]
    local indicator = item.current and '* ' or '  '
    local line = indicator .. item.name

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

      -- Highlight current colorscheme indicator
      if item.current then
        vim.api.nvim_buf_add_highlight(M.state.list_buf, M.state.ns_id, 'Conditional', line_idx - 1, 0, 1)
      end

      -- Sign for current colorscheme indicator
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

--- Apply colorscheme preview when cursor moves
function M.preview_colorscheme()
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

  -- Apply the colorscheme for preview
  pcall(vim.cmd, 'colorscheme ' .. item.name)
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
  M.preview_colorscheme()
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
  M.preview_colorscheme()
end

function M.select()
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

  local selected_colorscheme = item.name

  vim.cmd('stopinsert')
  M.close(true) -- Pass true to indicate successful selection

  -- Apply the selected colorscheme
  vim.cmd('colorscheme ' .. selected_colorscheme)
end

--- Close the picker
--- @param selected boolean|nil If true, user selected a colorscheme; if false/nil, restore original
function M.close(selected)
  if not M.state.active then
    return
  end

  vim.cmd('stopinsert')
  M.state.active = false

  local windows = { M.state.input_win, M.state.list_win }
  for _, win in ipairs(windows) do
    if win and vim.api.nvim_win_is_valid(win) then
      vim.api.nvim_win_close(win, true)
    end
  end

  local buffers = { M.state.input_buf, M.state.list_buf }
  for _, buf in ipairs(buffers) do
    if buf and vim.api.nvim_buf_is_valid(buf) then
      vim.api.nvim_buf_clear_namespace(buf, -1, 0, -1)
      vim.api.nvim_buf_delete(buf, { force = true })
    end
  end

  -- Restore original colorscheme if user cancelled
  if not selected and M.original_colorscheme then
    pcall(vim.cmd, 'colorscheme ' .. M.original_colorscheme)
  end

  -- Reset state
  M.state.input_win = nil
  M.state.list_win = nil
  M.state.input_buf = nil
  M.state.list_buf = nil
  M.state.items = {}
  M.state.filtered_items = {}
  M.state.cursor = 1
  M.state.query = ''
  M.state.ns_id = nil
  M.original_colorscheme = nil

  pcall(vim.api.nvim_del_augroup_by_name, 'fff_colors_picker_focus')
end

--- Open the colors picker
--- @param opts? table Optional configuration to override defaults
--- @param opts.bang? boolean If true, use fullscreen mode (no live preview)
function M.open(opts)
  if M.state.active then
    return
  end

  opts = opts or {}

  local config = conf.get()
  local merged_config = vim.tbl_deep_extend('force', config or {}, opts or {})

  -- Override prompt for colors picker
  if merged_config.prompt == nil or merged_config.prompt == config.prompt then
    merged_config.prompt = 'Colors> '
  end

  M.state.config = merged_config
  M.state.active = true

  -- Store original colorscheme for restoration on cancel
  M.original_colorscheme = vim.g.colors_name

  -- Get colorscheme items
  M.state.items = M.get_colorscheme_items()
  M.state.filtered_items = M.state.items

  if not M.create_ui() then
    vim.notify('Failed to create colors picker UI', vim.log.levels.ERROR)
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
  M.update_status()

  vim.cmd('startinsert!')
end

return M
