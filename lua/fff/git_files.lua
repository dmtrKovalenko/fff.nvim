--- FFF.nvim Git Files Picker - Similar to fzf.vim :GFiles? command
--- Lists and fuzzy-searches through git status files with preview

local M = {}

local conf = require('fff.conf')
local preview = require('fff.file_picker.preview')
local git_utils = require('fff.git_utils')

-- ============================================================================
-- Git Files Retrieval
-- ============================================================================

--- Get the git root directory
--- @return string|nil Git root path or nil if not in a git repo
function M.get_git_root()
  local result = vim.fn.system('git rev-parse --show-toplevel 2>/dev/null')
  if vim.v.shell_error ~= 0 then
    return nil
  end
  return result:gsub('\n', '')
end

--- Get git status files from git status output
--- @param git_root string Git root directory
--- @return table List of git files with status
function M.get_git_status_files(git_root)
  local handle = io.popen('git -C ' .. vim.fn.shellescape(git_root) .. ' status -s 2>/dev/null')
  if not handle then
    return {}
  end

  local files = {}
  local seen = {}

  for line in handle:lines() do
    if line:len() >= 3 then
      local status_code = line:sub(1, 2)
      local filepath = line:sub(4)

      -- Skip duplicates
      if not seen[filepath] then
        seen[filepath] = true

        -- Parse git status code
        local git_status = 'unknown'
        if status_code:match('[AM]%s') or status_code:match('%s[AM]') then
          git_status = 'staged_new'
        elseif status_code:match('[M]%s') or status_code:match('%s[M]') then
          if status_code:match('[M]%s') then
            git_status = 'staged_modified'
          else
            git_status = 'modified'
          end
        elseif status_code:match('D%s') or status_code:match('%sD') then
          git_status = 'deleted'
        elseif status_code:match('R%s') or status_code:match('%sR') then
          git_status = 'renamed'
        elseif status_code:match('??') then
          git_status = 'untracked'
        elseif status_code:match('!!') then
          git_status = 'ignored'
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
    M.state.ns_id = vim.api.nvim_create_namespace('fff_git_files_picker')
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
  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', false)

  if M.is_preview_enabled() then
    M.state.preview_buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_option(M.state.preview_buf, 'bufhidden', 'wipe')
    vim.api.nvim_buf_set_option(M.state.preview_buf, 'modifiable', false)
  end

  -- Create input window
  local input_height = prompt_position == 'bottom' and 1 or 1
  local input_row = prompt_position == 'bottom' and row + height - input_height or row
  local list_height = height - input_height - 1
  local list_row = prompt_position == 'bottom' and row or row + input_height + 1

  M.state.input_win = vim.api.nvim_open_win(M.state.input_buf, true, {
    relative = 'editor',
    width = list_width,
    height = input_height,
    row = input_row,
    col = col,
    border = 'single',
    title = config.prompt or '🦆 ',
    title_pos = 'left',
    zindex = 101,
  })

  vim.api.nvim_win_set_option(M.state.input_win, 'wrap', false)
  vim.api.nvim_win_set_option(M.state.input_win, 'list', false)

  -- Create list window
  M.state.list_win = vim.api.nvim_open_win(M.state.list_buf, false, {
    relative = 'editor',
    width = list_width,
    height = list_height,
    row = list_row,
    col = col,
    border = 'single',
    title = ' ' .. (config.title or 'Git Files') .. ' ',
    title_pos = 'left',
    zindex = 100,
  })

  vim.api.nvim_win_set_option(M.state.list_win, 'wrap', false)

  -- Create preview window if enabled
  if M.is_preview_enabled() then
    M.state.preview_win = vim.api.nvim_open_win(M.state.preview_buf, false, {
      relative = 'editor',
      width = preview_width,
      height = height,
      row = row,
      col = col + list_width + 2,
      border = 'single',
      title = ' Preview ',
      title_pos = 'left',
      zindex = 100,
    })

    vim.api.nvim_win_set_option(M.state.preview_win, 'wrap', config.preview.wrap_lines or false)
  end

  return true
end

function M.render_list()
  if not M.state.active or not M.state.list_buf then
    return
  end

  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', true)

  local lines = {}
  for i, item in ipairs(M.state.filtered_items) do
    local is_cursor = i == M.state.cursor
    local border_char = git_utils.get_border_char(item.git_status)
    local relative = item.relative_path

    if border_char and border_char ~= '' then
      relative = border_char .. ' ' .. relative
    end

    table.insert(lines, relative)
  end

  vim.api.nvim_buf_set_lines(M.state.list_buf, 0, -1, false, lines)
  vim.api.nvim_buf_set_option(M.state.list_buf, 'modifiable', false)

  -- Clear all highlights
  vim.api.nvim_buf_clear_namespace(M.state.list_buf, M.state.ns_id, 0, -1)

  -- Apply git status highlights
  for i, item in ipairs(M.state.filtered_items) do
    local is_cursor = i == M.state.cursor
    local border_hl = is_cursor
        and git_utils.get_border_highlight_selected(item.git_status)
        or git_utils.get_border_highlight(item.git_status)
    local text_hl = git_utils.get_text_highlight(item.git_status)

    if is_cursor and (text_hl == '' or not text_hl) then
      border_hl = 'CursorLine'
    end

    if border_hl and border_hl ~= '' then
      vim.api.nvim_buf_set_extmark(M.state.list_buf, M.state.ns_id, i - 1, 0, {
        line_hl_group = border_hl,
        priority = 10,
      })
    end
  end

  -- Highlight cursor line
  vim.api.nvim_win_set_cursor(M.state.list_win, { M.state.cursor, 0 })
end

function M.update_results()
  if not M.state.active then
    return
  end

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
  if not M.state.active then
    return
  end

  local query = M.state.query:lower()

  if query == '' then
    M.state.filtered_items = M.state.items
  else
    M.state.filtered_items = {}
    for _, item in ipairs(M.state.items) do
      if item.relative_path:lower():find(query, 1, true) then
        table.insert(M.state.filtered_items, item)
      end
    end
  end

  M.state.cursor = 1
end

function M.update_preview()
  if not M.state.active or not M.state.preview_win or not M.is_preview_enabled() then
    return
  end

  local items = M.state.filtered_items
  if #items == 0 or M.state.cursor > #items then
    M.clear_preview()
    return
  end

  local item = items[M.state.cursor]
  if not item or item.path == M.state.last_preview_file then
    return
  end

  M.state.last_preview_file = item.path

  if not preview.preview(item.path, M.state.preview_buf) then
    M.clear_preview()
  end
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
  if not M.state.active then
    return
  end

  local input_lines = vim.api.nvim_buf_get_lines(M.state.input_buf, 0, 1, false)
  M.state.query = input_lines[1] or ''

  M.filter_results()
  M.render_list()
  M.update_preview()
  M.update_status()
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

  pcall(vim.api.nvim_del_augroup_by_name, 'fff_git_files_picker_focus')
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

  if keymaps.preview_scroll_up then
    vim.keymap.set('i', keymaps.preview_scroll_up, M.scroll_preview_up, input_opts)
  end
  if keymaps.preview_scroll_down then
    vim.keymap.set('i', keymaps.preview_scroll_down, M.scroll_preview_down, input_opts)
  end
end

--- Open the git files picker
--- @param opts? table Optional configuration to override defaults
function M.open(opts)
  if M.state.active then
    return
  end

  local git_root = M.get_git_root()
  if not git_root then
    vim.notify('Not in a git repository', vim.log.levels.WARN)
    return
  end

  local config = conf.get()
  local merged_config = vim.tbl_deep_extend('force', config or {}, opts or {})

  if merged_config.title == nil then
    merged_config.title = 'Git Files'
  end
  if merged_config.prompt == nil then
    merged_config.prompt = '🦆 '
  end

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

  M.setup_keymaps()

  -- Set up autocmd for input changes
  local group = vim.api.nvim_create_augroup('fff_git_files_picker_focus', { clear = true })
  vim.api.nvim_create_autocmd('TextChangedI', {
    group = group,
    buffer = M.state.input_buf,
    callback = M.on_input_changed,
    desc = 'Update git files picker on input change',
  })

  vim.cmd('startinsert!')
end

return M
