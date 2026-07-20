package.loaded['fff'] = {
  find_files = function() end,
  live_grep = function() end,
}

package.loaded['fff.conf'] = {
  get = function()
    return {
      preview = {},
      layout = {},
      keymaps = {},
    }
  end,
}

package.loaded['fff.file_picker.preview'] = {
  setup = function() end,
}

package.loaded['fff.file_picker.icons'] = {
  get_icon = function() return '', 'Normal' end,
}

package.loaded['fff.utils'] = {
  normalize_path = function(path) return path end,
}

package.loaded['fff.highlights'] = {
  get_git_border_char = function() return ' ' end,
  get_git_border_highlight = function() return 'Normal' end,
  get_git_border_highlight_selected = function() return 'Visual' end,
  should_show_git_border = function() return false end,
}

local function test_extension_module_loads()
  print('Testing fff_plus module loads...')
  local plus = require('fff_plus')
  assert(plus, 'fff_plus module should load')
  assert(type(plus.setup) == 'function', 'fff_plus.setup should be a function')
  assert(type(plus.buffers) == 'function', 'fff_plus.buffers should be a function')
  assert(type(plus.colors) == 'function', 'fff_plus.colors should be a function')
  assert(type(plus.git_files) == 'function', 'fff_plus.git_files should be a function')
  print('✓ fff_plus module loads correctly')
end

local function test_picker_modules_load()
  print('Testing fff_plus picker modules load...')
  local buffers = require('fff_plus.pickers.buffers')
  local colors = require('fff_plus.pickers.colors')
  local git_files = require('fff_plus.pickers.git_files')

  assert(type(buffers.open) == 'function', 'buffers.open should be a function')
  assert(type(colors.open) == 'function', 'colors.open should be a function')
  assert(type(git_files.open) == 'function', 'git_files.open should be a function')
  assert(type(git_files.get_git_root) == 'function', 'git_files.get_git_root should be a function')
  print('✓ fff_plus picker modules load correctly')
end

local function test_fuzzy_matcher()
  print('Testing fuzzy matcher...')
  local matcher = require('fff_plus.matcher')

  assert(matcher.score('buffer_test_module.lua', 'btm'), 'subsequence queries should match')
  assert(matcher.score('buffer.lua', 'buf') > matcher.score('big_utility_file.lua', 'buf'))

  local items = {
    { name = 'big_utility_file.lua' },
    { name = 'buffer.lua' },
    { name = 'buffers.lua' },
  }
  local matches = matcher.filter(items, 'buf', function(item) return item.name end)

  assert(#matches == 3, 'all fuzzy matches should be returned')
  assert(matches[1].name == 'buffer.lua', 'stronger and shorter matches should rank first')

  local buffers = require('fff_plus.pickers.buffers')
  local buffer_matches = buffers.filter_buffers({ { display_name = 'buffer_test_module.lua' } }, 'btm')
  assert(#buffer_matches == 1, 'buffer picker should use fuzzy subsequence matching')

  local colors = require('fff_plus.pickers.colors')
  local color_matches = colors.filter_colorschemes({ { name = 'tokyonight-moon' } }, 'tnm')
  assert(#color_matches == 1, 'colors picker should use fuzzy subsequence matching')

  local git_files = require('fff_plus.pickers.git_files')
  git_files.state.active = true
  git_files.state.items = { { relative_path = 'lua/fff_plus/matcher.lua' } }
  git_files.state.query = 'fpm'
  git_files.filter_results()
  assert(#git_files.state.filtered_items == 1, 'Git picker should use fuzzy subsequence matching')
  git_files.state.active = false
  print('✓ fuzzy matcher ranks subsequence matches')
end

local function test_viewport_calculation()
  print('Testing picker viewport...')
  local viewport = require('fff_plus.viewport')

  local first_page = viewport.calculate(3, 3, 5, 'bottom')
  assert(first_page.first == 1 and first_page.last == 3)
  assert(first_page.padding == 2 and first_page.cursor_line == 5)

  local scrolled = viewport.calculate(20, 12, 5, 'bottom')
  assert(scrolled.first == 8 and scrolled.last == 12)
  assert(scrolled.padding == 0 and scrolled.cursor_line == 5)

  local top = viewport.calculate(20, 7, 5, 'top')
  assert(top.first == 3 and top.last == 7)
  assert(top.padding == 0 and top.cursor_line == 5)
  print('✓ picker viewport keeps the logical cursor visible')
end

local function test_git_sources()
  print('Testing Git sources...')
  local git_source = require('fff_plus.git_source')

  local tracked = git_source.parse_tracked('README.md\0lua/file with spaces.lua\0')
  assert(#tracked == 2, 'tracked parser should preserve every NUL-delimited path')
  assert(tracked[2] == 'lua/file with spaces.lua', 'tracked parser should preserve spaces')

  local status =
    git_source.parse_status(' M lua/modified file.lua\0R  lua/new name.lua\0lua/old name.lua\0?? lua/untracked.lua\0')
  assert(#status == 3, 'status parser should return modified, renamed, and untracked files')
  assert(status[1].git_status == 'modified')
  assert(status[1].relative_path == 'lua/modified file.lua')
  assert(status[2].git_status == 'renamed')
  assert(status[2].relative_path == 'lua/new name.lua')
  assert(status[2].old_path == 'lua/old name.lua')
  assert(status[3].git_status == 'untracked')
  print('✓ Git sources preserve paths and rename records')
end

local function test_picker_selection()
  print('Testing picker selection...')
  local selection = require('fff_plus.selection')
  local selected = {}

  assert(selection.toggle(selected, 'b.lua') == true)
  assert(selected['b.lua'] == true, 'toggle should select a new key')
  assert(selection.toggle(selected, 'b.lua') == false)
  assert(selected['b.lua'] == nil, 'toggle should clear an existing key')

  selected['b.lua'] = true
  selected['a.lua'] = true
  local items = { { path = 'a.lua' }, { path = 'b.lua' }, { path = 'c.lua' } }
  local chosen = selection.collect(items, selected, items[3], function(item) return item.path end)
  assert(#chosen == 2 and chosen[1].path == 'a.lua' and chosen[2].path == 'b.lua')

  local fallback = selection.collect(items, {}, items[3], function(item) return item.path end)
  assert(#fallback == 1 and fallback[1].path == 'c.lua', 'current item should be used with no selection')
  print('✓ picker selection preserves item order and current-item fallback')
end

local function test_commands_register()
  print('Testing fff_plus commands register...')
  require('fff_plus').setup()

  assert(vim.fn.exists(':FFFPlusBuffers') == 2, 'FFFPlusBuffers command should exist')
  assert(vim.fn.exists(':FFFPlusColors') == 2, 'FFFPlusColors command should exist')
  assert(vim.fn.exists(':FFFPlusGFiles') == 2, 'FFFPlusGFiles command should exist')
  assert(vim.fn.exists(':FFFPlusGitFiles') == 2, 'FFFPlusGitFiles command should exist')
  assert(vim.fn.exists(':FFFPlusGitStatus') == 2, 'FFFPlusGitStatus command should exist')
  assert(type(require('fff_plus').tracked_files) == 'function', 'tracked_files API should exist')
  assert(type(require('fff_plus').git_status) == 'function', 'git_status API should exist')
  print('✓ fff_plus commands register correctly')
end

local function run_tests()
  print('\n=== Running fff_plus extension tests ===\n')

  local tests = {
    test_extension_module_loads,
    test_picker_modules_load,
    test_fuzzy_matcher,
    test_viewport_calculation,
    test_git_sources,
    test_picker_selection,
    test_commands_register,
  }

  for _, test in ipairs(tests) do
    local ok, err = pcall(test)
    if not ok then
      print('✗ Test failed: ' .. tostring(err))
      return false
    end
  end

  print('\n=== All fff_plus extension tests passed ===\n')
  return true
end

if arg[0]:match('test_fff_plus_extension') then
  if not run_tests() then os.exit(1) end
end
