--- fff-plus.nvim Git Files Picker - Similar to fzf.vim :GFiles? command
--- Lists and fuzzy-searches through git status files with preview

local M = {}

local conf = require('fff.conf')
local preview = require('fff.file_picker.preview')
local git_utils = require('fff_plus.git_utils')
local matcher = require('fff_plus.matcher')
local viewport = require('fff_plus.viewport')

-- Initialize preview module with config (required before using preview functions)
local preview_config = conf.get().preview
if preview_config then preview.setup(preview_config) end

-- ============================================================================
-- Git Files Retrieval
-- ============================================================================

--- Get the git root directory
--- @return string|nil Git root path or nil if not in a git repo
function M.get_git_root()
  local result = vim.fn.system('git rev-parse --show-toplevel 2>/dev/null')
  if vim.v.shell_error ~= 0 then return nil end
  return result:gsub('\n', '')
end

--- Get git status files from git status output
--- @param git_root string Git root directory
--- @return table List of git files with status
function M.get_git_status_files(git_root)
  local handle = io.popen('git -C ' .. vim.fn.shellescape(git_root) .. ' status -s 2>/dev/null')
  if not handle then return {} end

  local files = {}
  local seen = {}

  for line in handle:lines() do
    if line:len() >= 3 then
      -- Git status format: XY filename
      -- X = index status, Y = worktree status
      local index_status = line:sub(1, 1)
      local worktree_status = line:sub(2, 2)
      local filepath = line:sub(4)

      -- Handle renamed files: R  old -> new
      if index_status == 'R' or worktree_status == 'R' then
        local arrow_pos = filepath:find(' %-> ')
        if arrow_pos then
          filepath = filepath:sub(arrow_pos + 4) -- Get the new filename
        end
      end

      -- Skip duplicates
      if not seen[filepath] then
        seen[filepath] = true

        -- Parse git status code (XY format)
        -- X = staged (index), Y = unstaged (worktree)
        local git_status = 'unknown'

        if index_status == '?' and worktree_status == '?' then
          git_status = 'untracked'
        elseif index_status == '!' and worktree_status == '!' then
          git_status = 'ignored'
        elseif index_status == 'A' then
          git_status = 'staged_new'
        elseif index_status == 'M' then
          git_status = 'staged_modified'
        elseif index_status == 'D' then
          git_status = 'staged_deleted'
        elseif index_status == 'R' then
          git_status = 'renamed'
        elseif worktree_status == 'M' then
          git_status = 'modified'
        elseif worktree_status == 'D' then
          git_status = 'deleted'
        elseif worktree_status == 'A' then
          git_status = 'untracked' -- Added in worktree only (like untracked)
        end

        local full_path = git_root .. '/' .. filepath
        local name = vim.fn.fnamemodify(filepath, ':t')
        local directory = vim.fn.fnamemodify(filepath, ':h')

        table.insert(files, {
          name = name,
          path = full_path,
          relative_path = filepath,
          directory = directory,
          extension = vim.fn.fnamemodify(name, ':e'),
          git_status = git_status,
          is_dir = false,
        })
      end
    end
  end

  handle:close()
  return files
end

-- ============================================================================
-- Git Files Picker UI (reuses picker_ui patterns)
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
  if config and config.layout and config.layout.prompt_position then return config.layout.prompt_position end
  return 'bottom'
end

function M.is_preview_enabled()
  local preview_state = nil
  if M.state.config and M.state.config.preview then preview_state = M.state.config.preview end
  if not preview_state then return true end
  return preview_state.enabled
end

function M.create_ui()
  local config = M.state.config

  if not M.state.ns_id then M.state.ns_id = vim.api.nvim_create_namespace('fff_plus_git_files_picker') end

  local terminal_width = vim.o.columns
  local terminal_height = vim.o.lines

  -- Calculate dimensions
  local width_ratio = config.layout.width or 0.8
  local height_ratio = config.layout.height or 0.8
  if type(width_ratio) == 'function' then width_ratio = width_ratio(terminal_width, terminal_height) end
  if type(height_ratio) == 'function' then height_ratio = height_ratio(terminal_width, terminal_height) end

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
    title = prompt_position == 'bottom' and ' Git Files ' or nil,
    title_pos = prompt_position == 'bottom' and 'left' or nil,
  })

  -- Create preview window if enabled
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
    title = prompt_position == 'top' and ' Git Files ' or nil,
    title_pos = prompt_position == 'top' and 'left' or nil,
  })

  M.setup_buffers()
  M.setup_windows()
  M.setup_keymaps()

  vim.api.nvim_set_current_win(M.state.input_win)

  if M.is_preview_enabled() then preview.set_preview_window(M.state.preview_win) end

  return true
end

function M.setup_buffers()
  vim.api.nvim_buf_set_name(M.state.input_buf, 'fff git files search')
  vim.api.nvim_buf_set_name(M.state.list_buf, 'fff git files list')
  if M.is_preview_enabled() then vim.api.nvim_buf_set_name(M.state.preview_buf, 'fff git files preview') end

  vim.api.nvim_buf_set_option(M.state.input_buf, 'buftype', 'prompt')
  vim.api.nvim_buf_set_option(M.state.input_buf, 'filetype', 'fff_plus_git_files_input')
  vim.fn.prompt_setprompt(M.state.input_buf, M.state.config.prompt or '🦆 ')

  vim.api.nvim_buf_set_option(M.state.list_buf, 'buftype', 'nofile')
  vim.api.nvim_buf_set_option(M.state.list_buf, 'filetype', 'fff_plus_git_files_list')
  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', false)

  if M.is_preview_enabled() then
    vim.api.nvim_buf_set_option(M.state.preview_buf, 'buftype', 'nofile')
    vim.api.nvim_buf_set_option(M.state.preview_buf, 'filetype', 'fff_plus_git_files_preview')
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
    vim.api.nvim_win_set_option(M.state.preview_win, 'wrap', M.state.config.preview.wrap_lines or false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'cursorline', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'number', false)
    vim.api.nvim_win_set_option(M.state.preview_win, 'winhighlight', win_hl)
  end

  -- Close picker when focus leaves
  local picker_group = vim.api.nvim_create_augroup('fff_plus_git_files_picker_focus', { clear = true })
  local picker_windows = { M.state.input_win, M.state.list_win }
  if M.state.preview_win then table.insert(picker_windows, M.state.preview_win) end

  vim.api.nvim_create_autocmd('WinLeave', {
    group = picker_group,
    callback = function()
      if not M.state.active then return end

      local current_win = vim.api.nvim_get_current_win()
      local is_picker_window = vim.tbl_contains(picker_windows, current_win)

      if is_picker_window then
        vim.defer_fn(function()
          if not M.state.active then return end

          local new_win = vim.api.nvim_get_current_win()
          if not vim.tbl_contains(picker_windows, new_win) then M.close() end
        end, 10)
      end
    end,
    desc = 'Close git files picker when focus leaves',
  })
end

function M.render_list()
  if not M.state.active or not M.state.list_buf then return end

  local items = M.state.filtered_items
  local win_height = vim.api.nvim_win_get_height(M.state.list_win)
  local win_width = vim.api.nvim_win_get_width(M.state.list_win)
  local prompt_position = get_prompt_position()
  local view = viewport.calculate(#items, M.state.cursor, win_height, prompt_position)
  local empty_lines_needed = view.padding
  local cursor_line = view.cursor_line

  local lines = {}

  -- Add empty lines for bottom prompt position
  if prompt_position == 'bottom' then
    for _ = 1, empty_lines_needed do
      table.insert(lines, string.rep(' ', win_width))
    end
  end

  -- Format each git file line
  for item_index = view.first, view.last do
    local item = items[item_index]
    local border_char = git_utils.get_border_char(item.git_status)
    local line = ''

    if border_char and border_char ~= '' then
      line = border_char .. ' ' .. item.relative_path
    else
      line = '  ' .. item.relative_path
    end

    -- Pad line
    local line_len = vim.fn.strdisplaywidth(line)
    local padding = math.max(0, win_width - line_len + 5)
    table.insert(lines, line .. string.rep(' ', padding))
  end

  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', true)
  vim.api.nvim_buf_set_lines(M.state.list_buf, 0, -1, false, lines)
  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', false)

  -- Clear all highlights
  vim.api.nvim_buf_clear_namespace(M.state.list_buf, M.state.ns_id, 0, -1)

  if #items > 0 and cursor_line > 0 and cursor_line <= #lines then
    vim.api.nvim_win_set_cursor(M.state.list_win, { cursor_line, 0 })

    -- Highlight cursor line
    vim.api.nvim_buf_add_highlight(M.state.list_buf, M.state.ns_id, M.state.config.hl.cursor, cursor_line - 1, 0, -1)

    -- Apply git status highlights for each visible item
    for item_index = view.first, view.last do
      local item = items[item_index]
      local line_idx = empty_lines_needed + item_index - view.first + 1
      local is_cursor_line = line_idx == cursor_line

      local border_hl = is_cursor_line and git_utils.get_border_highlight_selected(item.git_status)
        or git_utils.get_border_highlight(item.git_status)

      -- Add sign for git status
      if git_utils.should_show_border(item.git_status) then
        vim.api.nvim_buf_set_extmark(M.state.list_buf, M.state.ns_id, line_idx - 1, 0, {
          sign_text = git_utils.get_border_char(item.git_status),
          sign_hl_group = border_hl,
          priority = 1000,
        })
      end
    end
  end
end

function M.update_results()
  if not M.state.active then return end

  local git_root = M.get_git_root()
  if not git_root then
    M.state.items = {}
    M.state.filtered_items = {}
    M.render_list()
    M.update_status()
    return
  end

  M.state.items = M.get_git_status_files(git_root)
  M.filter_results()
  M.render_list()
  M.update_status()
end

function M.filter_results()
  if not M.state.active then return end

  M.state.filtered_items = matcher.filter(
    M.state.items,
    M.state.query,
    function(item) return item.relative_path or '' end
  )

  M.state.cursor = 1
end

function M.update_preview()
  if not M.is_preview_enabled() then return end
  if not M.state.active then return end

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

  if M.state.last_preview_file == item.path then return end

  preview.clear()
  M.state.last_preview_file = item.path

  -- Update preview window title
  local title = string.format(' %s ', item.relative_path or item.name)
  vim.api.nvim_win_set_config(M.state.preview_win, {
    title = title,
    title_pos = 'left',
  })

  preview.set_preview_window(M.state.preview_win)
  preview.preview(item.path, M.state.preview_buf)
end

function M.clear_preview()
  if not M.state.active then return end
  if not M.is_preview_enabled() then return end

  vim.api.nvim_win_set_config(M.state.preview_win, {
    title = ' Preview ',
    title_pos = 'left',
  })

  vim.api.nvim_buf_set_option(M.state.preview_buf, 'modifiable', true)
  vim.api.nvim_buf_set_lines(M.state.preview_buf, 0, -1, false, { 'No preview available' })
  vim.api.nvim_buf_set_option(M.state.preview_buf, 'modifiable', false)
end

function M.update_status()
  if not M.state.active or not M.state.ns_id then return end

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
  if not M.state.active then return end
  if #M.state.filtered_items == 0 then return end

  M.state.cursor = math.max(M.state.cursor - 1, 1)
  M.render_list()
  M.update_preview()
end

function M.move_down()
  if not M.state.active then return end
  if #M.state.filtered_items == 0 then return end

  M.state.cursor = math.min(M.state.cursor + 1, #M.state.filtered_items)
  M.render_list()
  M.update_preview()
end

function M.scroll_preview_up()
  if not M.state.active or not M.state.preview_win then return end
  local win_height = vim.api.nvim_win_get_height(M.state.preview_win)
  preview.scroll(-math.floor(win_height / 2))
end

function M.scroll_preview_down()
  if not M.state.active or not M.state.preview_win then return end
  local win_height = vim.api.nvim_win_get_height(M.state.preview_win)
  preview.scroll(math.floor(win_height / 2))
end

function M.select(action)
  if not M.state.active then return end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then return end

  local item = items[M.state.cursor]
  if not item then return end

  action = action or 'edit'

  vim.cmd('stopinsert')
  M.close()

  if action == 'edit' then
    vim.cmd('edit ' .. vim.fn.fnameescape(item.path))
  elseif action == 'split' then
    vim.cmd('split ' .. vim.fn.fnameescape(item.path))
  elseif action == 'vsplit' then
    vim.cmd('vsplit ' .. vim.fn.fnameescape(item.path))
  elseif action == 'tab' then
    vim.cmd('tabnew ' .. vim.fn.fnameescape(item.path))
  end
end

function M.on_input_changed()
  if not M.state.active then return end

  local lines = vim.api.nvim_buf_get_lines(M.state.input_buf, 0, -1, false)
  local prompt_len = #(M.state.config.prompt or '🦆 ')
  local query = ''

  local full_line = lines[1] or ''
  if full_line:sub(1, prompt_len) == (M.state.config.prompt or '🦆 ') then query = full_line:sub(prompt_len + 1) end

  M.state.query = query
  M.filter_results()
  M.render_list()
  M.update_preview()
  M.update_status()
end

function M.close()
  if not M.state.active then return end

  vim.cmd('stopinsert')
  M.state.active = false

  local windows = { M.state.input_win, M.state.list_win, M.state.preview_win }
  for _, win in ipairs(windows) do
    if win and vim.api.nvim_win_is_valid(win) then vim.api.nvim_win_close(win, true) end
  end

  local buffers = { M.state.input_buf, M.state.list_buf }
  if M.is_preview_enabled() then table.insert(buffers, M.state.preview_buf) end

  for _, buf in ipairs(buffers) do
    if buf and vim.api.nvim_buf_is_valid(buf) then
      vim.api.nvim_buf_clear_namespace(buf, -1, 0, -1)
      if buf == M.state.preview_buf then preview.clear_buffer(buf) end
      vim.api.nvim_buf_delete(buf, { force = true })
    end
  end

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

  pcall(vim.api.nvim_del_augroup_by_name, 'fff_plus_git_files_picker_focus')
end

function M.setup_keymaps()
  local keymaps = M.state.config.keymaps

  local input_opts = { buffer = M.state.input_buf, noremap = true, silent = true }

  vim.keymap.set('i', keymaps.close, M.close, input_opts)
  vim.keymap.set('i', keymaps.select, function() M.select('edit') end, input_opts)
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

  if keymaps.preview_scroll_up then vim.keymap.set('i', keymaps.preview_scroll_up, M.scroll_preview_up, input_opts) end
  if keymaps.preview_scroll_down then
    vim.keymap.set('i', keymaps.preview_scroll_down, M.scroll_preview_down, input_opts)
  end

  -- Handle input changes using buf_attach
  vim.api.nvim_buf_attach(M.state.input_buf, false, {
    on_lines = function()
      vim.schedule(function() M.on_input_changed() end)
    end,
  })
end

--- Open the git files picker
--- @param opts? table Optional configuration to override defaults
function M.open(opts)
  if M.state.active then return end

  local git_root = M.get_git_root()
  if not git_root then
    vim.notify('Not in a git repository', vim.log.levels.WARN)
    return
  end

  local config = conf.get()
  local merged_config = vim.tbl_deep_extend('force', config or {}, opts or {})

  if merged_config.title == nil then merged_config.title = 'Git Files' end
  if merged_config.prompt == nil then merged_config.prompt = '🦆 ' end

  M.state.config = merged_config
  M.state.active = true

  M.state.items = M.get_git_status_files(git_root)
  M.state.filtered_items = M.state.items

  if not M.create_ui() then
    vim.notify('Failed to create git files picker UI', vim.log.levels.ERROR)
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

return M
