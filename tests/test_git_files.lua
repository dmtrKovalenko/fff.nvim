-- Basic tests for git_files module

-- Note: These are basic existence and syntax tests
-- Full functional testing requires a Neovim instance

local function test_module_loads()
  print('Testing git_files module loads...')
  local git_files = require('fff.pickers.git_files')
  assert(git_files, 'git_files module should load')
  assert(type(git_files.open) == 'function', 'git_files.open should be a function')
  assert(type(git_files.get_git_root) == 'function', 'git_files.get_git_root should be a function')
  assert(type(git_files.get_git_status_files) == 'function', 'git_files.get_git_status_files should be a function')
  print('✓ git_files module loads correctly')
end

local function test_main_function_exists()
  print('Testing git_files function in main module...')
  local main = require('fff.main')
  assert(main.git_files, 'git_files function should exist in main module')
  assert(type(main.git_files) == 'function', 'git_files should be a function')
  print('✓ git_files function exists in main module')
end

-- Run tests
local function run_tests()
  print('\n=== Running git_files Tests ===\n')

  local tests = {
    test_module_loads,
    test_main_function_exists,
  }

  for _, test in ipairs(tests) do
    local ok, err = pcall(test)
    if not ok then
      print('✗ Test failed: ' .. tostring(err))
      return false
    end
  end

  print('\n=== All Tests Passed ===\n')
  return true
end

if arg[0]:match('test_git_files') then run_tests() end
