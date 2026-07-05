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

local function test_commands_register()
  print('Testing fff_plus commands register...')
  require('fff_plus').setup()

  assert(vim.fn.exists(':FFFPlusBuffers') == 2, 'FFFPlusBuffers command should exist')
  assert(vim.fn.exists(':FFFPlusColors') == 2, 'FFFPlusColors command should exist')
  assert(vim.fn.exists(':FFFPlusGFiles') == 2, 'FFFPlusGFiles command should exist')
  print('✓ fff_plus commands register correctly')
end

local function run_tests()
  print('\n=== Running fff_plus extension tests ===\n')

  local tests = {
    test_extension_module_loads,
    test_picker_modules_load,
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
