package.loaded['fff.conf'] = {
  get = function()
    return {
      prompt = '> ',
      layout = {},
      preview = { enabled = false },
      keymaps = {},
      hl = {},
    }
  end,
}

local picker = require('fff_plus.picker')

local confirmed
local instance = picker.create({
  name = 'memory',
  items = function()
    return {
      { id = 1, text = 'alpha.lua' },
      { id = 2, text = 'buffer_test.lua' },
      { id = 3, text = 'buffers.lua' },
    }
  end,
  key = function(item) return item.id end,
  text = function(item) return item.text end,
  confirm = function(_, item, action) confirmed = { item = item, action = action } end,
}, {})

instance:refresh()
assert(instance:count() == 3, 'refresh should collect source items')

instance:set_query('btl')
assert(instance:count() == 1, 'query should fuzzy-filter source items')
assert(instance:current().text == 'buffer_test.lua', 'current should return the ranked item')

instance:toggle_selection()
assert(#instance:selected() == 1, 'toggle_selection should select the current item')
assert(instance:selected()[1].id == 2, 'selected items should retain source order')

instance:confirm('split')
assert(confirmed.item.id == 2, 'confirm should receive the current item')
assert(confirmed.action == 'split', 'confirm should receive the requested action')

instance:set_query('missing')
assert(instance:current() == nil, 'current should be nil when the picker is empty')
assert(#instance:selected({ fallback = true }) == 1, 'explicit selections should survive filtering')

print('Shared picker core tests passed')

local ui = picker.pick({
  name = 'ui-memory',
  title = 'Memory',
  items = function()
    return {
      { id = 1, text = 'one.lua' },
      { id = 2, text = 'two.lua' },
    }
  end,
  key = function(item) return item.id end,
  text = function(item) return item.text end,
  format = function(item) return item.text end,
}, { enter = false, layout = { prompt_position = 'top' } })

assert(ui.active == true, 'pick should open an active picker')
assert(vim.api.nvim_win_is_valid(ui.input_win), 'pick should create an input window')
assert(vim.api.nvim_win_is_valid(ui.list_win), 'pick should create a list window')
assert(vim.api.nvim_buf_get_lines(ui.list_buf, 0, -1, false)[1] == 'one.lua', 'pick should render source items')

local input_win = ui.input_win
local list_win = ui.list_win
ui:close(false)
assert(not vim.api.nvim_win_is_valid(input_win), 'close should remove the input window')
assert(not vim.api.nvim_win_is_valid(list_win), 'close should remove the list window')

print('Shared picker UI smoke test passed')

local custom_action_item
local preview_ui = picker.pick({
  name = 'preview-memory',
  items = function()
    return {
      { id = 1, text = 'first' },
      { id = 2, text = 'second' },
    }
  end,
  preview = function(_, item) return { title = item.text, lines = { 'preview ' .. item.text }, filetype = 'text' } end,
  actions = {
    custom = function(_, item) custom_action_item = item end,
  },
}, { enter = false, layout = { prompt_position = 'top' }, preview = { enabled = true } })

assert(vim.api.nvim_win_is_valid(preview_ui.preview_win), 'preview sources should create a preview window')
assert(
  vim.api.nvim_buf_get_lines(preview_ui.preview_buf, 0, -1, false)[1] == 'preview first',
  'preview should render the current item'
)
preview_ui:action('custom')
assert(custom_action_item.id == 1, 'custom actions should receive the current item')
preview_ui:move('down')
assert(
  vim.api.nvim_buf_get_lines(preview_ui.preview_buf, 0, -1, false)[1] == 'preview second',
  'moving should update the preview'
)
preview_ui:close(false)

print('Shared picker preview and action tests passed')
