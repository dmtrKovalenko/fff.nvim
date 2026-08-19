local picker = require('fff_plus.picker')
local shared = require('fff_plus.sources.shared')

local M = {}

local function buffer_name(bufnr)
  local path = vim.api.nvim_buf_get_name(bufnr)
  return path == '' and '[No Name]' or vim.fn.fnamemodify(path, ':~:.')
end

function M.collect(opts)
  opts = opts or {}
  local buffers = {}
  if (opts.scope or 'current') == 'current' then
    buffers = { opts.bufnr or vim.api.nvim_get_current_buf() }
  else
    for _, bufnr in ipairs(vim.api.nvim_list_bufs()) do
      if vim.api.nvim_buf_is_loaded(bufnr) and vim.fn.buflisted(bufnr) == 1 then table.insert(buffers, bufnr) end
    end
  end

  local items = {}
  for _, bufnr in ipairs(buffers) do
    local path = vim.api.nvim_buf_get_name(bufnr)
    local name = buffer_name(bufnr)
    for line, text in ipairs(vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)) do
      if text:find('%S') then
        table.insert(items, {
          bufnr = bufnr,
          path = path,
          relative_path = name,
          line = line,
          col = 1,
          text = text,
          display = string.format('%s:%d  %s', name, line, text),
        })
      end
    end
  end
  return items
end

function M.create(opts)
  opts = vim.tbl_deep_extend('force', { scope = 'current', prompt = 'Lines> ' }, opts or {})
  if opts.title == nil then opts.title = opts.scope == 'loaded' and 'Loaded Buffer Lines' or 'Buffer Lines' end
  local instance = picker.create({
    name = 'lines',
    title = opts.title,
    resume = function(resume_opts) return M.open(resume_opts) end,
    items = function() return M.collect(opts) end,
    key = function(item) return string.format('%d:%d', item.bufnr, item.line) end,
    text = function(item) return item.text end,
    fields = function(item) return { buffer = item.relative_path, path = item.path, line = tostring(item.line) } end,
    format = function(item) return item.display end,
    preview = shared.preview_buffer,
    confirm = function(_, item, action) return shared.jump(item, action) end,
    actions = {
      qflist = function(ctx) shared.send_list(ctx, 'FFF+ Lines', false) end,
      loclist = function(ctx) shared.send_list(ctx, 'FFF+ Lines', true) end,
    },
  }, opts)
  instance.origin_win = vim.api.nvim_get_current_win()
  return instance
end

function M.open(opts)
  if M.state and M.state.active then return M.state end
  M.state = M.create(opts)
  return M.state:open()
end

return M
