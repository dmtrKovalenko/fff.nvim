local matcher = require('fff_plus.matcher')
local selection = require('fff_plus.selection')

local M = {}
local Picker = {}
Picker.__index = Picker

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

function Picker:refresh()
  local items = self.spec.items(self)
  self.items = type(items) == 'table' and items or {}
  self.cursor = 1
  self:apply_query()
  return self.items
end

function Picker:set_query(query)
  self.query = tostring(query or '')
  self.cursor = 1
  self:apply_query()
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
  return selection.toggle(self.selected_keys, self:key(item))
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
  return not all_selected
end

function Picker:confirm(action)
  local item = self:current()
  if not item or not self.spec.confirm then return false end
  self.spec.confirm(self, item, action or 'edit')
  return true
end

function Picker:close(confirmed)
  if self.closed then return end
  self.closed = true
  if self.spec.on_close then self.spec.on_close(self, confirmed == true) end
end

function M.create(spec, opts)
  validate_spec(spec)
  opts = opts or {}

  local instance = setmetatable({
    spec = spec,
    opts = opts,
    items = {},
    filtered_items = {},
    selected_keys = {},
    query = tostring(opts.query or ''),
    cursor = 1,
    closed = false,
  }, Picker)

  return instance
end

function M.pick(spec, opts)
  local instance = M.create(spec, opts)
  instance:refresh()
  M.last = instance
  return instance
end

return M
