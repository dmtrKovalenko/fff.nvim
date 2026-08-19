package.loaded['fff.conf'] = {
  get = function() return { prompt = '> ', layout = {}, preview = { enabled = true }, keymaps = {}, hl = {} } end,
}

local layout = require('fff_plus.layout')

local left = layout.windows(
  { width = 100, height = 30, col = 10, row = 4 },
  { prompt_position = 'top', preview_position = 'left', preview_size = 0.4 },
  true
)
assert(left.has_preview and left.preview.col == 10, 'left layouts should place the preview first')
assert(left.list.col > left.preview.col, 'left layouts should place results after the preview')

local top = layout.windows(
  { width = 100, height = 30, col = 10, row = 4 },
  { prompt_position = 'top', preview_position = 'top', preview_size = 0.4 },
  true
)
assert(top.preview.row == 4 and top.list.row > top.preview.row, 'top layouts should stack preview over results')

local compact = layout.windows(
  { width = 50, height = 20, col = 0, row = 0 },
  { preview_position = 'right', preview_min_width = 70 },
  true
)
assert(not compact.has_preview, 'responsive layouts should hide preview when space is constrained')

local picker = require('fff_plus.picker')
picker.history = {}
picker.snapshots = {}

local spec = {
  name = 'history-memory',
  items = function() return { { text = 'alpha' }, { text = 'beta' } } end,
  preview = function(_, item) return { lines = { item.text } } end,
}

local first = picker.create(spec, { enter = false })
first:set_query('alpha')
first:close(false)
local second = picker.create(spec, { enter = false })
second:set_query('beta')
second:close(false)

local browsing = picker.create(spec, { enter = false })
assert(browsing:history_previous() == 'beta', 'history should start with the newest query')
assert(browsing:history_previous() == 'alpha', 'history should walk toward older queries')
assert(browsing:history_next() == 'beta', 'history should walk toward newer queries')

local help = browsing:help_lines()
assert(table.concat(help, '\n'):find('refresh', 1, true), 'help should describe shared actions')

local resumed = picker.resume('history-memory', { enter = false })
assert(resumed and resumed.active and resumed.query == 'beta', 'resume should reopen the latest picker snapshot')
assert(vim.api.nvim_win_is_valid(resumed.preview_win), 'resumed picker should restore its preview')
resumed:toggle_preview()
assert(resumed.preview_win == nil, 'preview toggle should remove the preview window')
resumed:toggle_preview()
assert(vim.api.nvim_win_is_valid(resumed.preview_win), 'preview toggle should restore the preview window')
resumed:toggle_maximize()
assert(resumed.maximized == true, 'maximize should be shared picker state')
resumed:focus('list')
assert(vim.api.nvim_get_current_win() == resumed.list_win, 'focus actions should switch picker panes')
resumed:close(false)

print('Picker history, resume, help, focus, and responsive layout tests passed')
