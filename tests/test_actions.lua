package.loaded['fff.conf'] = {
  get = function() return { prompt = '> ', layout = {}, preview = { enabled = false }, keymaps = {}, hl = {} } end,
}

local actions = require('fff_plus.actions')
local picker = require('fff_plus.picker')

local refreshed = 0
local context = { refresh = function() refreshed = refreshed + 1 end }
assert(actions.run(context, 'refresh') == true, 'the registry should expose shared actions')
assert(refreshed == 1, 'the shared refresh action should call the picker')

local shared_item
actions.register('remember', function(_, item) shared_item = item end)

local local_item
local instance = picker.create({
  name = 'actions-memory',
  items = function() return { { id = 1, text = 'one' } } end,
  actions = { remember = function(_, item) local_item = item end },
}, {})
instance:refresh()
assert(instance:action('remember') == true, 'picker actions should resolve through the registry')
assert(local_item.id == 1 and shared_item == nil, 'source actions should override shared actions')
assert(instance:action('missing') == false, 'unknown actions should report that they were not handled')

print('Shared action registry tests passed')
