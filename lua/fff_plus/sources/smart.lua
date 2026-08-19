local icons = require('fff.file_picker.icons')
local picker = require('fff_plus.picker')
local shared = require('fff_plus.sources.shared')

local M = {}

local function normalize(path)
  if not path or path == '' then return nil end
  return vim.fs.normalize(vim.fn.fnamemodify(path, ':p'))
end

local function source_label(sources)
  local labels = {}
  if sources.buffer then table.insert(labels, 'B') end
  if sources.recent then table.insert(labels, 'R') end
  if sources.indexed then table.insert(labels, 'F') end
  return table.concat(labels, '')
end

function M.combine(buffer_items, recent_paths, indexed_items)
  local entries = {}

  local function entry_for(path, data)
    path = normalize(path)
    if not path then return nil end
    local entry = entries[path]
    if not entry then
      entry = {
        path = path,
        name = vim.fn.fnamemodify(path, ':t'),
        extension = vim.fn.fnamemodify(path, ':e'),
        relative_path = data.relative_path or vim.fn.fnamemodify(path, ':~:.'),
        sources = {},
        total_frecency_score = 0,
      }
      entries[path] = entry
    end
    return entry
  end

  for index, item in ipairs(buffer_items or {}) do
    local entry = entry_for(item.path, item)
    if entry then
      entry.sources.buffer = true
      entry.bufnr = item.bufnr
      entry.line = item.line or 1
      entry._buffer_rank = math.max(entry._buffer_rank or 0, 400 - index)
    end
  end

  for index, path in ipairs(recent_paths or {}) do
    local entry = entry_for(path, {})
    if entry then
      entry.sources.recent = true
      entry._recent_rank = math.max(entry._recent_rank or 0, 200 - index)
    end
  end

  for index, item in ipairs(indexed_items or {}) do
    local entry = entry_for(item.path, item)
    if entry then
      entry.sources.indexed = true
      entry.total_frecency_score = math.max(entry.total_frecency_score, item.total_frecency_score or 0)
      entry._indexed_rank = math.max(entry._indexed_rank or 0, 100 - index)
      entry.git_status = item.git_status
    end
  end

  local combined = {}
  for _, entry in pairs(entries) do
    entry.kind = entry.sources.buffer and 'buffer' or (entry.sources.recent and 'recent' or 'indexed')
    entry.source = source_label(entry.sources)
    entry.rank = entry.total_frecency_score * 1000
      + (entry._buffer_rank or 0)
      + (entry._recent_rank or 0)
      + (entry._indexed_rank or 0)
    table.insert(combined, entry)
  end
  table.sort(combined, function(left, right)
    if left.rank == right.rank then return left.relative_path < right.relative_path end
    return left.rank > right.rank
  end)
  return combined
end

local function collect_buffers(opts)
  if opts.buffer_items then return opts.buffer_items end
  return require('fff_plus.pickers.buffers').get_buffer_items()
end

local function collect_recent(opts)
  if opts.recent_files then return opts.recent_files end
  local paths = {}
  for _, path in ipairs(vim.v.oldfiles or {}) do
    if vim.fn.filereadable(path) == 1 then table.insert(paths, path) end
  end
  return paths
end

local function collect_indexed(opts)
  if opts.indexed_files then return opts.indexed_files end
  local loaded, fff = pcall(require, 'fff')
  if not loaded or type(fff.file_search) ~= 'function' then return {} end
  local ok, result = pcall(fff.file_search, '', {
    cwd = opts.cwd,
    max_results = opts.max_results or 200,
    wait_for_index_ms = opts.wait_for_index_ms or 0,
  })
  return ok and result and result.items or {}
end

function M.collect(opts)
  opts = opts or {}
  return M.combine(collect_buffers(opts), collect_recent(opts), collect_indexed(opts))
end

local function load_items(instance, done)
  local job = { cancelled = false }
  function job:cancel() self.cancelled = true end
  vim.schedule(function()
    if not job.cancelled then done(M.collect(instance.opts)) end
  end)
  return job
end

local function format_item(item)
  local icon, icon_hl = icons.get_icon(item.name, item.extension, false)
  local prefix = string.format('[%s] %s ', item.source, icon or '')
  return {
    text = prefix .. item.relative_path,
    highlights = { { group = icon_hl or 'Normal', start = #item.source + 3, finish = #prefix - 1 } },
    match_offset = #prefix,
  }
end

function M.create(opts)
  opts =
    vim.tbl_deep_extend('force', { title = 'Smart', prompt = 'Smart> ', keymaps = { paste = '<A-CR>' } }, opts or {})
  local instance = picker.create({
    name = 'smart',
    title = 'Smart',
    items = load_items,
    key = function(item) return item.path end,
    text = function(item) return item.relative_path end,
    fields = function(item) return { kind = item.kind, source = item.source, path = item.path } end,
    format = format_item,
    preview = shared.preview_file,
    confirm = function(_, item, action) return shared.jump(item, action) end,
    actions = {
      qflist = function(ctx) shared.send_list(ctx, 'FFF+ Smart', false) end,
      loclist = function(ctx) shared.send_list(ctx, 'FFF+ Smart', true) end,
      paste = shared.paste,
    },
    on_close = shared.close_preview,
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
