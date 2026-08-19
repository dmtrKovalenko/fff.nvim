local actions = require('fff_plus.actions')
local matcher = require('fff_plus.matcher')
local layout = require('fff_plus.layout')
local selection = require('fff_plus.selection')
local viewport = require('fff_plus.viewport')

local M = {}
local Picker = {}
Picker.__index = Picker
M.active = {}
M.next_id = 0

local function default_key(item) return item.id or item.path or item.text or tostring(item) end

local function default_text(item)
  return item.text or item.relative_path or item.display_name or item.name or tostring(item)
end

local function validate_spec(spec)
  vim.validate({
    spec = { spec, 'table' },
    name = { spec.name, 'string' },
    items = { spec.items, 'function' },
    key = { spec.key, 'function', true },
    text = { spec.text, 'function', true },
    confirm = { spec.confirm, 'function', true },
  })
end

function Picker:key(item) return (self.spec.key or default_key)(item, self) end

function Picker:text(item) return (self.spec.text or default_text)(item, self) end

function Picker:apply_query()
  self.filtered_items = matcher.filter(self.items, self.query, function(item) return self:text(item) end)
  self.cursor = math.min(math.max(1, self.cursor), math.max(1, #self.filtered_items))
end

function Picker:changed()
  if self.active then
    self:render()
    self:update_status()
    self:update_preview()
  end
  if self.spec.on_change then self.spec.on_change(self, self:current()) end
end

function Picker:refresh()
  if self.source_job then
    local cancel = self.source_job.cancel or self.source_job.kill
    if cancel then pcall(cancel, self.source_job, 15) end
    self.source_job = nil
  end

  self.source_generation = self.source_generation + 1
  local generation = self.source_generation
  local completed = false
  local function done(items)
    completed = true
    if self.closed or generation ~= self.source_generation then return end
    self.source_job = nil
    self.items = type(items) == 'table' and items or {}
    self.cursor = 1
    self:apply_query()
    self:changed()
  end

  local result = self.spec.items(self, done)
  if type(result) == 'table' and (result.cancel or result.kill) then
    if not completed then self.source_job = result end
  else
    done(result)
  end
  return self.items
end

function Picker:set_query(query)
  self.query = tostring(query or '')
  self.cursor = 1
  self:apply_query()
  self:changed()
end

function Picker:count() return #self.filtered_items end

function Picker:current() return self.filtered_items[self.cursor] end

function Picker:selected(opts)
  opts = opts or {}
  local current = opts.fallback and self:current() or nil
  return selection.collect(self.items, self.selected_keys, current, function(item) return self:key(item) end)
end

function Picker:toggle_selection()
  local item = self:current()
  if not item then return false end
  local selected = selection.toggle(self.selected_keys, self:key(item))
  if self.active then self:render() end
  return selected
end

function Picker:select_all()
  local all_selected = #self.filtered_items > 0
  for _, item in ipairs(self.filtered_items) do
    if not self.selected_keys[self:key(item)] then
      all_selected = false
      break
    end
  end

  for _, item in ipairs(self.filtered_items) do
    local key = self:key(item)
    self.selected_keys[key] = all_selected and nil or true
  end
  if self.active then self:render() end
  return not all_selected
end

function Picker:move(direction)
  if #self.filtered_items == 0 then return end
  local prompt_position = self.config.layout.prompt_position or 'bottom'
  self.cursor = viewport.move(self.cursor, #self.filtered_items, direction, prompt_position)
  self:changed()
end

function Picker:confirm(action)
  local item = self:current()
  if not item or not self.spec.confirm then return false end
  local after_close = self.spec.confirm(self, item, action or 'edit')
  if self.active then self:close(true) end
  if type(after_close) == 'function' then after_close() end
  return true
end

function Picker:action(name, ...) return actions.run(self, name, self.spec.actions, ...) end

function Picker:close(confirmed)
  if self.closed then return end
  self.active = false
  self.closed = true

  if self.preview_job then
    local cancel = self.preview_job.cancel or self.preview_job.kill
    if cancel then pcall(cancel, self.preview_job, 15) end
    self.preview_job = nil
  end
  if self.source_job then
    local cancel = self.source_job.cancel or self.source_job.kill
    if cancel then pcall(cancel, self.source_job, 15) end
    self.source_job = nil
  end

  if self.spec.on_close then self.spec.on_close(self, confirmed == true) end

  for _, win in ipairs({ self.input_win, self.list_win, self.preview_win }) do
    if win and vim.api.nvim_win_is_valid(win) then vim.api.nvim_win_close(win, true) end
  end
  for _, buf in ipairs({ self.input_buf, self.list_buf, self.preview_buf }) do
    if buf and vim.api.nvim_buf_is_valid(buf) then vim.api.nvim_buf_delete(buf, { force = true }) end
  end

  pcall(vim.api.nvim_del_augroup_by_name, self.augroup_name)
  if M.active[self.spec.name] == self then M.active[self.spec.name] = nil end
end

local function highlight(config, name, fallback) return config.hl and config.hl[name] or fallback end

function Picker:create_ui()
  local frame = layout.frame(vim.o.columns, vim.o.lines, self.config.layout, self.config.fullscreen)
  local prompt_position = self.config.layout.prompt_position or 'bottom'
  local list_height = math.max(1, frame.height - 3)
  local has_preview = self.spec.preview and self.config.preview.enabled ~= false
  local preview_width = has_preview and math.max(1, math.floor(frame.width * (self.config.layout.preview_size or 0.5)))
    or 0
  local picker_width = math.max(1, frame.width - preview_width - (has_preview and 2 or 0))

  self.input_buf = vim.api.nvim_create_buf(false, true)
  self.list_buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_name(self.input_buf, string.format('fff-plus %s input %d', self.spec.name, self.id))
  vim.api.nvim_buf_set_name(self.list_buf, string.format('fff-plus %s list %d', self.spec.name, self.id))

  vim.bo[self.input_buf].buftype = 'prompt'
  vim.bo[self.input_buf].filetype = 'fff_plus_input'
  vim.fn.prompt_setprompt(self.input_buf, self.config.prompt)
  vim.bo[self.list_buf].buftype = 'nofile'
  vim.bo[self.list_buf].filetype = 'fff_plus_list'
  vim.bo[self.list_buf].modifiable = false

  if has_preview then
    self.preview_buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_name(self.preview_buf, string.format('fff-plus %s preview %d', self.spec.name, self.id))
    vim.bo[self.preview_buf].buftype = 'nofile'
    vim.bo[self.preview_buf].filetype = 'fff_plus_preview'
    vim.bo[self.preview_buf].modifiable = false
  end

  local list_row = prompt_position == 'bottom' and frame.row + 1 or frame.row + 2
  local input_row = prompt_position == 'bottom' and frame.row + list_height + 2 or frame.row + 1

  local list_config = {
    relative = 'editor',
    width = picker_width,
    height = list_height,
    col = frame.col,
    row = list_row,
    border = 'single',
    style = 'minimal',
  }
  if prompt_position == 'bottom' then
    list_config.title = ' ' .. self.config.title .. ' '
    list_config.title_pos = 'left'
  end
  self.list_win = vim.api.nvim_open_win(self.list_buf, false, list_config)

  local input_config = {
    relative = 'editor',
    width = picker_width,
    height = 1,
    col = frame.col,
    row = input_row,
    border = 'single',
    style = 'minimal',
  }
  if prompt_position == 'top' then
    input_config.title = ' ' .. self.config.title .. ' '
    input_config.title_pos = 'left'
  end
  self.input_win = vim.api.nvim_open_win(self.input_buf, false, input_config)

  if has_preview then
    self.preview_win = vim.api.nvim_open_win(self.preview_buf, false, {
      relative = 'editor',
      width = preview_width,
      height = frame.height,
      col = frame.col + picker_width + 2,
      row = frame.row,
      border = 'single',
      style = 'minimal',
      title = ' Preview ',
      title_pos = 'left',
    })
  end

  local win_hl = string.format(
    'Normal:%s,FloatBorder:%s,FloatTitle:%s',
    highlight(self.config, 'normal', 'NormalFloat'),
    highlight(self.config, 'border', 'FloatBorder'),
    highlight(self.config, 'title', 'FloatTitle')
  )
  vim.wo[self.input_win].winhighlight = win_hl
  vim.wo[self.list_win].winhighlight = win_hl
  vim.wo[self.list_win].signcolumn = 'yes:1'
  vim.wo[self.list_win].cursorline = false
  if self.preview_win then
    vim.wo[self.preview_win].winhighlight = win_hl
    vim.wo[self.preview_win].wrap = self.config.preview.wrap_lines or false
  end

  self:setup_keymaps()
  self:setup_input_listener()
end

function Picker:format(item)
  if not self.spec.format then return self:text(item) end
  return self.spec.format(item, self)
end

function Picker:render()
  if not self.active or not self.list_win or not vim.api.nvim_win_is_valid(self.list_win) then return end

  local height = vim.api.nvim_win_get_height(self.list_win)
  local width = vim.api.nvim_win_get_width(self.list_win)
  local prompt_position = self.config.layout.prompt_position or 'bottom'
  local view = viewport.calculate(#self.filtered_items, self.cursor, height, prompt_position)
  local first = view.reverse and view.last or view.first
  local last = view.reverse and view.first or view.last
  local step = view.reverse and -1 or 1
  local lines = {}
  local rendered = {}

  for _ = 1, view.padding do
    table.insert(lines, string.rep(' ', width))
  end
  for index = first, last, step do
    local formatted = self:format(self.filtered_items[index])
    local text = type(formatted) == 'table' and formatted.text or formatted
    table.insert(lines, tostring(text or ''))
    rendered[#lines] = {
      item = self.filtered_items[index],
      highlights = type(formatted) == 'table' and formatted.highlights or nil,
      sign = type(formatted) == 'table' and formatted.sign or nil,
    }
  end
  if #lines == 0 then lines = { self.config.empty_text or 'No results' } end

  vim.bo[self.list_buf].modifiable = true
  vim.api.nvim_buf_set_lines(self.list_buf, 0, -1, false, lines)
  vim.bo[self.list_buf].modifiable = false
  vim.api.nvim_buf_clear_namespace(self.list_buf, self.ns_id, 0, -1)

  for line, value in pairs(rendered) do
    for _, value_hl in ipairs(value.highlights or {}) do
      vim.api.nvim_buf_add_highlight(
        self.list_buf,
        self.ns_id,
        value_hl.group,
        line - 1,
        value_hl.start or 0,
        value_hl.finish or -1
      )
    end

    local sign = value.sign
    if self.selected_keys[self:key(value.item)] then sign = { text = '▊', hl = 'Visual' } end
    if sign and sign.text and sign.text ~= '' then
      vim.api.nvim_buf_set_extmark(self.list_buf, self.ns_id, line - 1, 0, {
        sign_text = sign.text,
        sign_hl_group = sign.hl or 'Normal',
        priority = sign.priority or 1000,
      })
    end
  end

  if #self.filtered_items > 0 and view.cursor_line > 0 then
    vim.api.nvim_win_set_cursor(self.list_win, { view.cursor_line, 0 })
    vim.api.nvim_buf_add_highlight(
      self.list_buf,
      self.ns_id,
      highlight(self.config, 'cursor', 'Visual'),
      view.cursor_line - 1,
      0,
      -1
    )
  end
end

function Picker:apply_preview(result)
  if not self.preview_buf or not vim.api.nvim_buf_is_valid(self.preview_buf) then return end
  result = result or {}
  local lines = result.lines or { result.text or 'No preview available' }

  vim.bo[self.preview_buf].modifiable = true
  vim.api.nvim_buf_set_lines(self.preview_buf, 0, -1, false, lines)
  if result.filetype then vim.bo[self.preview_buf].filetype = result.filetype end
  vim.bo[self.preview_buf].modifiable = false

  if result.title and self.preview_win and vim.api.nvim_win_is_valid(self.preview_win) then
    vim.api.nvim_win_set_config(self.preview_win, { title = ' ' .. result.title .. ' ', title_pos = 'left' })
  end
end

function Picker:update_preview()
  if not self.active or not self.preview_buf or not self.spec.preview then return end
  self.preview_generation = self.preview_generation + 1
  local generation = self.preview_generation
  local item = self:current()
  if not item then
    self:apply_preview()
    return
  end

  if self.preview_job then
    local cancel = self.preview_job.cancel or self.preview_job.kill
    if cancel then pcall(cancel, self.preview_job, 15) end
    self.preview_job = nil
  end

  local function done(result)
    vim.schedule(function()
      if self.active and generation == self.preview_generation then self:apply_preview(result) end
    end)
  end

  local ok, result = pcall(self.spec.preview, self, item, done)
  if not ok then
    done({ lines = { 'Preview failed: ' .. tostring(result) }, filetype = 'text' })
  elseif type(result) == 'table' and (result.lines or result.text) then
    self:apply_preview(result)
  elseif type(result) == 'table' then
    self.preview_job = result
  end
end

function Picker:update_status()
  if not self.active or not self.input_win or not vim.api.nvim_win_is_valid(self.input_win) then return end
  local status = string.format('%d/%d', #self.filtered_items, #self.items)
  local width = vim.api.nvim_win_get_width(self.input_win)
  vim.api.nvim_buf_clear_namespace(self.input_buf, self.ns_id, 0, -1)
  vim.api.nvim_buf_set_extmark(self.input_buf, self.ns_id, 0, 0, {
    virt_text = { { status, 'LineNr' } },
    virt_text_win_col = math.max(0, width - #status - 2),
  })
end

function Picker:setup_keymaps()
  local keys = self.config.keymaps
  local opts = { buffer = self.input_buf, noremap = true, silent = true }
  local function set(key, callback)
    if key then vim.keymap.set('i', key, callback, opts) end
  end

  set(keys.close or '<Esc>', function() self:close(false) end)
  set(keys.select or '<CR>', function() self:confirm('edit') end)
  set(keys.select_split or '<C-s>', function() self:confirm('split') end)
  set(keys.select_vsplit or '<C-v>', function() self:confirm('vsplit') end)
  set(keys.select_tab or '<C-t>', function() self:confirm('tab') end)
  set(keys.toggle_select or '<Tab>', function() self:toggle_selection() end)
  set(keys.send_to_quickfix or '<C-q>', function() self:action('qflist') end)
  set(keys.paste, function() self:action('paste') end)
  set(keys.preview_scroll_up, function() self:action('preview_scroll_up') end)
  set(keys.preview_scroll_down, function() self:action('preview_scroll_down') end)

  for name, key in pairs(self.spec.keymaps or {}) do
    set(key, function() self:action(name) end)
  end

  local up = type(keys.move_up) == 'table' and keys.move_up or { keys.move_up or '<C-p>' }
  local down = type(keys.move_down) == 'table' and keys.move_down or { keys.move_down or '<C-n>' }
  for _, key in ipairs(up) do
    set(key, function() self:move('up') end)
  end
  for _, key in ipairs(down) do
    set(key, function() self:move('down') end)
  end
end

function Picker:setup_input_listener()
  vim.api.nvim_buf_attach(self.input_buf, false, {
    on_lines = function()
      vim.schedule(function()
        if not self.active or not vim.api.nvim_buf_is_valid(self.input_buf) then return end
        local line = vim.api.nvim_buf_get_lines(self.input_buf, 0, 1, false)[1] or ''
        local prompt = self.config.prompt
        if line:sub(1, #prompt) == prompt then line = line:sub(#prompt + 1) end
        self:set_query(line)
      end)
    end,
  })
end

function Picker:open()
  if self.active then return self end
  self.closed = false
  self.active = true
  self:refresh()
  self:create_ui()
  self:render()
  self:update_status()
  self:update_preview()
  M.active[self.spec.name] = self
  M.last = self

  if self.opts.enter ~= false then
    vim.api.nvim_set_current_win(self.input_win)
    vim.cmd('startinsert!')
  end
  return self
end

function M.create(spec, opts)
  validate_spec(spec)
  opts = opts or {}

  local config = vim.tbl_deep_extend('force', require('fff.conf').get() or {}, opts)
  config.layout = config.layout or {}
  config.keymaps = config.keymaps or {}
  config.preview = config.preview or {}
  config.prompt = config.prompt or '> '
  config.title = config.title or spec.title or spec.name

  M.next_id = M.next_id + 1
  local instance = setmetatable({
    id = M.next_id,
    ns_id = vim.api.nvim_create_namespace(string.format('fff_plus_picker_%d', M.next_id)),
    spec = spec,
    opts = opts,
    config = config,
    items = {},
    filtered_items = {},
    selected_keys = {},
    query = tostring(opts.query or ''),
    cursor = 1,
    closed = false,
    preview_generation = 0,
    source_generation = 0,
  }, Picker)

  return instance
end

function M.pick(spec, opts)
  local instance = M.create(spec, opts)
  return instance:open()
end

return M
