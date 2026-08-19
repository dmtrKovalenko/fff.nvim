--- Buffer source adapter for the shared fff-plus picker.

local icons = require('fff.file_picker.icons')
local preview = require('fff.file_picker.preview')
local matcher = require('fff_plus.matcher')
local picker = require('fff_plus.picker')
local selection = require('fff_plus.selection')

local M = { buffer_access_times = {} }

function M.setup_tracking()
  local group = vim.api.nvim_create_augroup('fff_plus_buffer_tracking', { clear = true })
  vim.api.nvim_create_autocmd({ 'BufWinEnter', 'WinEnter' }, {
    group = group,
    callback = function()
      local bufnr = vim.api.nvim_get_current_buf()
      if vim.fn.buflisted(bufnr) == 1 then M.buffer_access_times[bufnr] = vim.uv.hrtime() end
    end,
    desc = 'Track buffer access time for FFF+',
  })
  vim.api.nvim_create_autocmd('BufDelete', {
    group = group,
    callback = function(args) M.buffer_access_times[args.buf] = nil end,
    desc = 'Forget deleted FFF+ buffers',
  })
end

function M.get_listed_buffers()
  local buffers = {}
  for _, bufnr in ipairs(vim.api.nvim_list_bufs()) do
    if vim.fn.buflisted(bufnr) == 1 and vim.bo[bufnr].buftype ~= 'quickfix' then table.insert(buffers, bufnr) end
  end
  return buffers
end

function M.sort_by_access(buffers)
  table.sort(buffers, function(left, right)
    local left_info = (vim.fn.getbufinfo(left) or {})[1] or {}
    local right_info = (vim.fn.getbufinfo(right) or {})[1] or {}
    local left_time = M.buffer_access_times[left] or left_info.lastused or 0
    local right_time = M.buffer_access_times[right] or right_info.lastused or 0
    if left_time == right_time then return left < right end
    return left_time > right_time
  end)
  return buffers
end

function M.format_buffer(bufnr)
  local info = (vim.fn.getbufinfo(bufnr) or {})[1] or {}
  local path = vim.api.nvim_buf_get_name(bufnr)
  local display_name = path == '' and '[No Name]' or vim.fn.fnamemodify(path, ':~:.')
  local name = path == '' and '[No Name]' or vim.fn.fnamemodify(path, ':t')
  local current = bufnr == vim.api.nvim_get_current_buf()
  local alternate = bufnr == vim.fn.bufnr('#')

  return {
    bufnr = bufnr,
    name = name,
    path = path,
    relative_path = display_name,
    display_name = display_name,
    directory = path == '' and '' or vim.fn.fnamemodify(path, ':h'),
    extension = path == '' and '' or vim.fn.fnamemodify(path, ':e'),
    line = math.max(info.lnum or 1, 1),
    modified = vim.bo[bufnr].modified,
    readonly = not vim.bo[bufnr].modifiable,
    current = current,
    alternate = alternate,
    status = current and '%' or (alternate and '#' or ''),
    is_dir = false,
  }
end

function M.get_buffer_items()
  local items = {}
  for _, bufnr in ipairs(M.sort_by_access(M.get_listed_buffers())) do
    table.insert(items, M.format_buffer(bufnr))
  end
  return items
end

function M.filter_buffers(items, query)
  return matcher.filter(items, query, function(item) return item.display_name or '' end)
end

function M.find_existing_window(bufnr, instance)
  local config = instance and instance.config or (M.state and M.state.config)
  if not (config and config.jump_to_existing) then return nil end
  for _, win in ipairs(vim.fn.win_findbuf(bufnr)) do
    if vim.api.nvim_win_is_valid(win) then return win end
  end
  return nil
end

local function format_item(item)
  local icon, icon_hl = icons.get_icon(item.name, item.extension, false)
  local status = item.status ~= '' and item.status .. ' ' or '  '
  local flags = (item.modified and '[+]' or '') .. (item.readonly and '[RO]' or '')
  if flags ~= '' then flags = ' ' .. flags end
  local prefix = string.format('[%d] %s%s ', item.bufnr, status, icon)
  local text = prefix .. item.display_name .. flags
  local highlights = { { group = icon_hl or 'Normal', start = #string.format('[%d] %s', item.bufnr, status) } }
  return {
    text = text,
    highlights = highlights,
    match_offset = #prefix,
    sign = item.current and { text = '▎', hl = 'Conditional' } or nil,
  }
end

local function preview_item(instance, item)
  if item.path == '' then
    return {
      title = item.display_name,
      lines = vim.api.nvim_buf_get_lines(item.bufnr, 0, -1, false),
      filetype = vim.bo[item.bufnr].filetype,
    }
  end

  if preview.setup then preview.setup(instance.config.preview) end
  if preview.set_preview_window then preview.set_preview_window(instance.preview_win) end
  if preview.preview then preview.preview(item.path, instance.preview_buf) end
  vim.api.nvim_win_set_config(instance.preview_win, { title = ' ' .. item.display_name .. ' ', title_pos = 'left' })
end

local function chosen(instance) return instance:selected({ fallback = true }) end

local function send_list(instance, location)
  local entries = {}
  for _, item in ipairs(chosen(instance)) do
    table.insert(entries, {
      bufnr = item.bufnr,
      lnum = item.line,
      col = 1,
      text = item.display_name,
    })
  end
  if #entries == 0 then return end

  instance:close(true)
  if location then
    vim.fn.setloclist(0, {}, ' ', { title = 'FFF+ Buffers', items = entries })
    vim.cmd('lopen')
  else
    vim.fn.setqflist({}, ' ', { title = 'FFF+ Buffers', items = entries })
    vim.cmd('copen')
  end
end

local function paste(instance)
  local items = chosen(instance)
  local origin = instance.origin_win
  instance:close(true)
  if origin and vim.api.nvim_win_is_valid(origin) then vim.api.nvim_set_current_win(origin) end
  selection.put(items, function(item) return item.path ~= '' and item.path or item.display_name end)
end

local function delete(instance, item)
  if not item then return end
  if #M.get_listed_buffers() <= 1 then
    vim.notify('Cannot delete the last buffer', vim.log.levels.WARN)
    return
  end
  if item.modified then
    vim.notify('Buffer has unsaved changes. Save first or use :bd!', vim.log.levels.WARN)
    return
  end
  pcall(vim.api.nvim_buf_delete, item.bufnr, {})
  instance:refresh()
end

function M.create(opts)
  opts = vim.tbl_deep_extend('force', {
    title = 'Buffers',
    prompt = '🦆 ',
    keymaps = { paste = '<A-CR>' },
  }, opts or {})

  local instance = picker.create({
    name = 'buffers',
    title = 'Buffers',
    items = function() return M.get_buffer_items() end,
    key = function(item) return item.bufnr end,
    text = function(item) return item.display_name end,
    format = format_item,
    preview = preview_item,
    confirm = function(ctx, item, action)
      local existing = action == 'edit' and M.find_existing_window(item.bufnr, ctx) or nil
      return function()
        if existing then
          vim.api.nvim_set_current_win(existing)
        elseif action == 'split' then
          vim.cmd('sbuffer ' .. item.bufnr)
        elseif action == 'vsplit' then
          vim.cmd('vertical sbuffer ' .. item.bufnr)
        elseif action == 'tab' then
          vim.cmd('tab sbuffer ' .. item.bufnr)
        else
          vim.cmd('buffer ' .. item.bufnr)
        end
        if item.line > 0 then pcall(vim.api.nvim_win_set_cursor, 0, { item.line, 0 }) end
      end
    end,
    actions = {
      qflist = function(ctx) send_list(ctx, false) end,
      loclist = function(ctx) send_list(ctx, true) end,
      paste = paste,
      delete = delete,
      preview_scroll_up = function(_, _)
        if preview.scroll then preview.scroll(-10) end
      end,
      preview_scroll_down = function(_, _)
        if preview.scroll then preview.scroll(10) end
      end,
    },
    keymaps = { delete = '<C-d>' },
    on_close = function(ctx)
      if ctx.preview_buf and preview.clear_buffer then preview.clear_buffer(ctx.preview_buf) end
    end,
  }, opts)
  instance.origin_win = vim.api.nvim_get_current_win()
  return instance
end

function M.open(opts)
  if M.state and M.state.active then return M.state end
  M.state = M.create(opts)
  return M.state:open()
end

function M.send_to_quickfix() return M.state and M.state:action('qflist') end

function M.paste_selection() return M.state and M.state:action('paste') end

function M.delete_buffer() return M.state and M.state:action('delete') end

function M.close() return M.state and M.state:close(false) end

M.setup_tracking()

return M
