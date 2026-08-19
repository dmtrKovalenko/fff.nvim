package.loaded['fff'] = {
  file_search = function() return { items = {} } end,
}
package.loaded['fff.conf'] = {
  get = function() return { prompt = '> ', layout = {}, preview = { enabled = false }, keymaps = {}, hl = {} } end,
}
package.loaded['fff.file_picker.preview'] = { setup = function() end }
package.loaded['fff.file_picker.icons'] = { get_icon = function() return '', 'Normal' end }

local smart = require('fff_plus.sources.smart')
local combined = smart.combine(
  { { path = '/repo/a.lua', bufnr = 1, line = 4 }, { path = '/repo/b.lua', bufnr = 2, line = 1 } },
  { '/repo/a.lua', '/repo/c.lua' },
  {
    { path = '/repo/a.lua', relative_path = 'a.lua', total_frecency_score = 7 },
    { path = '/repo/d.lua', relative_path = 'd.lua', total_frecency_score = 3 },
  }
)
assert(#combined == 4, 'smart should deduplicate normalized paths across providers')
assert(combined[1].path == '/repo/a.lua', 'smart should retain the strongest merged frecency metadata')
assert(combined[1].bufnr == 1 and combined[1].total_frecency_score == 7)
assert(combined[1].sources.buffer and combined[1].sources.recent and combined[1].sources.indexed)

local lines = require('fff_plus.sources.lines')
local first = vim.api.nvim_create_buf(true, false)
vim.api.nvim_buf_set_name(first, '/tmp/fff-plus-lines-one.lua')
vim.api.nvim_buf_set_lines(first, 0, -1, false, { 'local one = 1', '', 'return one' })
local second = vim.api.nvim_create_buf(true, false)
vim.api.nvim_buf_set_name(second, '/tmp/fff-plus-lines-two.lua')
vim.api.nvim_buf_set_lines(second, 0, -1, false, { 'local two = 2' })

local current_lines = lines.collect({ scope = 'current', bufnr = first })
assert(#current_lines == 2 and current_lines[2].line == 3, 'current lines should omit blank lines and retain positions')
local loaded_lines = lines.collect({ scope = 'loaded' })
local saw_second = false
for _, item in ipairs(loaded_lines) do
  if item.bufnr == second then saw_second = true end
end
assert(saw_second, 'loaded lines should include other loaded listed buffers')

local diagnostics = require('fff_plus.sources.diagnostics')
local diagnostic_items = diagnostics.collect({
  scope = 'workspace',
  get = function()
    return {
      { bufnr = first, lnum = 4, col = 2, severity = vim.diagnostic.severity.WARN, message = 'warning' },
      { bufnr = second, lnum = 1, col = 0, severity = vim.diagnostic.severity.ERROR, message = 'error' },
    }
  end,
})
assert(#diagnostic_items == 2 and diagnostic_items[1].severity_name == 'error')
assert(
  diagnostic_items[1].line == 2 and diagnostic_items[1].col == 1,
  'diagnostics should convert positions to one-based'
)

assert(smart.create({ preview = { enabled = false } }).spec.name == 'smart')
local lines_picker = lines.create({ preview = { enabled = false } })
assert(lines_picker.spec.name == 'lines' and lines_picker.spec.text(current_lines[1]) == current_lines[1].text)
local diagnostics_picker = diagnostics.create({ preview = { enabled = false } })
assert(
  diagnostics_picker.spec.name == 'diagnostics'
    and diagnostics_picker.spec.text(diagnostic_items[1]) == diagnostic_items[1].message
)

print('Smart, lines, and diagnostics source tests passed')
