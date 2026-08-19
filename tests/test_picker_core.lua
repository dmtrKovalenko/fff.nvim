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
