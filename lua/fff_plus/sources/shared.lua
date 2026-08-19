local preview = require('fff.file_picker.preview')
local selection = require('fff_plus.selection')

local M = {}

function M.preview_buffer(_, item)
  if not item.bufnr or not vim.api.nvim_buf_is_valid(item.bufnr) then return nil end
  return {
    title = item.relative_path or item.path or vim.api.nvim_buf_get_name(item.bufnr),
    lines = vim.api.nvim_buf_get_lines(item.bufnr, 0, -1, false),
    filetype = vim.bo[item.bufnr].filetype,
    cursor = { math.max(1, item.line or 1), math.max(0, (item.col or 1) - 1) },
  }
end

function M.preview_file(instance, item)
  if item.bufnr and vim.api.nvim_buf_is_valid(item.bufnr) then return M.preview_buffer(instance, item) end
  if preview.setup then preview.setup(instance.config.preview) end
  if preview.set_preview_window then preview.set_preview_window(instance.preview_win) end
  if preview.preview then preview.preview(item.path, instance.preview_buf) end
  if instance.preview_win and vim.api.nvim_win_is_valid(instance.preview_win) then
    vim.api.nvim_win_set_config(instance.preview_win, {
      title = ' ' .. (item.relative_path or item.path) .. ' ',
      title_pos = 'left',
    })
  end
end

function M.jump(item, action)
  return function()
    local bufnr = item.bufnr and vim.api.nvim_buf_is_valid(item.bufnr) and item.bufnr or nil
    if bufnr then
      local command = action == 'split' and 'sbuffer'
        or (action == 'vsplit' and 'vertical sbuffer' or (action == 'tab' and 'tab sbuffer' or 'buffer'))
      vim.cmd(command .. ' ' .. bufnr)
    else
      local command = action == 'split' and 'split'
        or (action == 'vsplit' and 'vsplit' or (action == 'tab' and 'tabnew' or 'edit'))
      vim.cmd(command .. ' ' .. vim.fn.fnameescape(item.path))
    end
    if item.line then pcall(vim.api.nvim_win_set_cursor, 0, { item.line, math.max(0, (item.col or 1) - 1) }) end
  end
end

function M.chosen(instance) return instance:selected({ fallback = true }) end

function M.send_list(instance, title, location)
  local entries = {}
  for _, item in ipairs(M.chosen(instance)) do
    table.insert(entries, {
      bufnr = item.bufnr,
      filename = item.bufnr and nil or item.path,
      lnum = item.line or 1,
      col = item.col or 1,
      text = item.message or item.text or item.relative_path,
      type = item.severity_letter,
    })
  end
  if #entries == 0 then return end

  instance:close(true)
  if location then
    vim.fn.setloclist(0, {}, ' ', { title = title, items = entries })
    vim.cmd('lopen')
  else
    vim.fn.setqflist({}, ' ', { title = title, items = entries })
    vim.cmd('copen')
  end
end

function M.paste(instance)
  local items = M.chosen(instance)
  local origin = instance.origin_win
  instance:close(true)
  if origin and vim.api.nvim_win_is_valid(origin) then vim.api.nvim_set_current_win(origin) end
  selection.put(items, function(item) return item.path or item.relative_path or item.text end)
end

function M.close_preview(instance)
  if instance.preview_buf and preview.clear_buffer then preview.clear_buffer(instance.preview_buf) end
end

return M
