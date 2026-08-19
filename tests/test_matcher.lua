local matcher = require('fff_plus.matcher')

assert(matcher.match('README.md', 'readme'), 'lowercase queries should match without case sensitivity')
assert(not matcher.match('README.md', 'Readme'), 'uppercase queries should preserve case')

assert(matcher.match('lua/fff_plus/picker.lua', "'fff_plus"), 'exact terms should match contiguous text')
assert(not matcher.match('lua/fff_plus/picker.lua', "'fplus"), 'exact terms should not use fuzzy matching')
assert(matcher.match('lua/fff_plus/picker.lua', '^lua'), 'prefix terms should anchor at the beginning')
assert(not matcher.match('lua/fff_plus/picker.lua', '^fff'), 'prefix terms should reject later matches')
assert(matcher.match('lua/fff_plus/picker.lua', '.lua$'), 'suffix terms should anchor at the end')
assert(not matcher.match('lua/fff_plus/picker.lua', '.md$'), 'suffix terms should reject other endings')
assert(matcher.match('lua/fff_plus/picker.lua', 'picker !test'), 'inverse terms should retain non-matches')
assert(not matcher.match('lua/fff_plus/test_picker.lua', 'picker !test'), 'inverse terms should reject matches')

local positional = matcher.match('abcdef', 'ace')
assert(vim.deep_equal(positional.positions, { 1, 3, 5 }), 'matches should expose one-based character positions')

local items = {
  { text = 'init.lua', kind = 'buffer' },
  { text = 'init.lua', kind = 'recent' },
  { text = 'README.md', kind = 'buffer' },
}
local filtered = matcher.filter(
  items,
  'kind:buffer init',
  function(item) return item.text end,
  function(item) return { kind = item.kind } end
)
assert(#filtered == 1 and filtered[1] == items[1], 'field filters should compose with text terms')

print('Advanced matcher tests passed')
