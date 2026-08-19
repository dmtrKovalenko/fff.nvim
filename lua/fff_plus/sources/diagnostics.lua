local picker = require('fff_plus.picker')
local shared = require('fff_plus.sources.shared')

local M = {}

local severity = {
  [vim.diagnostic.severity.ERROR] = { name = 'error', letter = 'E', hl = 'DiagnosticError' },
  [vim.diagnostic.severity.WARN] = { name = 'warn', letter = 'W', hl = 'DiagnosticWarn' },
  [vim.diagnostic.severity.INFO] = { name = 'info', letter = 'I', hl = 'DiagnosticInfo' },
  [vim.diagnostic.severity.HINT] = { name = 'hint', letter = 'H', hl = 'DiagnosticHint' },
}

function M.collect(opts)
  opts = opts or {}
  local bufnr = opts.bufnr or vim.api.nvim_get_current_buf()
  local get = opts.get or vim.diagnostic.get
  local diagnostics = get((opts.scope or 'workspace') == 'buffer' and bufnr or nil)
  local items = {}

  for _, diagnostic in ipairs(diagnostics) do
    local diagnostic_buf = diagnostic.bufnr or bufnr
    local info = severity[diagnostic.severity] or { name = 'unknown', letter = '?', hl = 'DiagnosticInfo' }
    local path = vim.api.nvim_buf_is_valid(diagnostic_buf) and vim.api.nvim_buf_get_name(diagnostic_buf) or ''
    local relative_path = path == '' and '[No Name]' or vim.fn.fnamemodify(path, ':~:.')
    local line = (diagnostic.lnum or 0) + 1
    local col = (diagnostic.col or 0) + 1
    table.insert(items, {
      bufnr = diagnostic_buf,
      path = path,
      relative_path = relative_path,
      line = line,
      col = col,
      message = tostring(diagnostic.message or ''):gsub('\n', ' '),
      source = diagnostic.source,
      code = diagnostic.code,
      severity = diagnostic.severity or math.huge,
      severity_name = info.name,
      severity_letter = info.letter,
      severity_hl = info.hl,
    })
  end

  table.sort(items, function(left, right)
    if left.severity ~= right.severity then return left.severity < right.severity end
    if left.relative_path ~= right.relative_path then return left.relative_path < right.relative_path end
    if left.line ~= right.line then return left.line < right.line end
    return left.col < right.col
  end)
  return items
end

local function format_item(item)
  local prefix = string.format('%s %s:%d:%d  ', item.severity_letter, item.relative_path, item.line, item.col)
  return {
    text = prefix .. item.message,
    highlights = { { group = item.severity_hl, start = 0, finish = 1 } },
    match_offset = #prefix,
  }
end

function M.create(opts)
  opts = vim.tbl_deep_extend('force', { scope = 'workspace', prompt = 'Diagnostics> ' }, opts or {})
  if opts.title == nil then opts.title = opts.scope == 'buffer' and 'Buffer Diagnostics' or 'Workspace Diagnostics' end
  local instance = picker.create({
    name = 'diagnostics',
    title = opts.title,
    items = function() return M.collect(opts) end,
    key = function(item) return string.format('%d:%d:%d:%s', item.bufnr, item.line, item.col, item.message) end,
    text = function(item) return item.message end,
    fields = function(item)
      return {
        severity = item.severity_name,
        source = item.source or '',
        code = tostring(item.code or ''),
        path = item.path,
      }
    end,
    format = format_item,
    preview = shared.preview_buffer,
    confirm = function(_, item, action) return shared.jump(item, action) end,
    actions = {
      qflist = function(ctx) shared.send_list(ctx, 'FFF+ Diagnostics', false) end,
      loclist = function(ctx) shared.send_list(ctx, 'FFF+ Diagnostics', true) end,
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
