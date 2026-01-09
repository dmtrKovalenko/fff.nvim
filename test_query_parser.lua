-- Test script for query parser and multi-part fuzzy matching
-- Run with: nvim -l test_query_parser.lua

-- Add the plugin to runtime path
vim.opt.runtimepath:prepend(vim.fn.expand('~/dev/fff.nvim'))

-- Set config for test
vim.g.fff = {
  base_path = vim.fn.expand('~/dev/lightsource'),
  frecency = { enabled = false },
  history = { enabled = false },
  logging = { enabled = false },
  watch_filesystem = false,
}

-- Initialize via the proper API
local fff = require('fff')
local core = require('fff.core')
local fuzzy = core.ensure_initialized()

print('Waiting for indexing...')

-- Use os.execute for blocking sleep since vim.wait doesn't work in -l mode
local function sleep_ms(ms)
  os.execute('sleep ' .. (ms / 1000))
end

local total_files = 0
for i = 1, 150 do  -- Wait up to 15 seconds
  local ok, progress = pcall(fuzzy.get_scan_progress)
  if ok and progress then
    -- Check is_scanning (false when done)
    if not progress.is_scanning and progress.scanned_files_count > 0 then
      total_files = progress.scanned_files_count
      print(string.format('Indexed %d files in ~%.1f seconds', total_files, i / 10))
      break
    elseif i % 10 == 0 then
      print(string.format('  Still indexing... %d files so far (is_scanning=%s)', 
        progress.scanned_files_count or 0, tostring(progress.is_scanning)))
    end
  end
  sleep_ms(100)
end

if total_files == 0 then
  local ok, progress = pcall(fuzzy.get_scan_progress)
  if ok and progress then
    total_files = progress.scanned_files_count or 0
    print(string.format('Final state: %d files, is_scanning=%s', 
      total_files, tostring(progress.is_scanning)))
  end
  if total_files == 0 then
    print('WARNING: No files indexed!')
  end
end

-- Test helper function
local function test_query(query, expected_behavior)
  print(string.format('\n--- Testing query: "%s" ---', query))
  print('Expected: ' .. expected_behavior)
  
  local results = fff.search(query, 20)
  print(string.format('Got %d results', #results))
  
  if #results > 0 then
    print('Top 5 results:')
    for i = 1, math.min(5, #results) do
      local r = results[i]
      print(string.format('  %d. %s (score: %d)', i, r.relative_path or r.path, r.score or 0))
    end
  end
  
  return results
end

print('\n========== QUERY PARSER TESTS ==========\n')

-- Test 1: Single plain text (should use raw fuzzy matching)
test_query('config', 'Should match files containing "config"')

-- Test 2: Single extension constraint
local rs_results = test_query('*.rs', 'Should only return .rs files')
-- Verify all results are .rs files
local rs_count = 0
for _, r in ipairs(rs_results) do
  if (r.relative_path or r.path):match('%.rs$') then
    rs_count = rs_count + 1
  else
    print('  ERROR: Non-.rs file in results: ' .. (r.relative_path or r.path))
  end
end
if #rs_results > 0 then
  print(string.format('  Verification: %d/%d are .rs files', rs_count, #rs_results))
end

-- Test 3: Multi-part fuzzy query (Nucleo-style)
test_query('api service', 'Should match files containing BOTH "api" AND "service"')

-- Test 4: Extension with fuzzy text
local py_config_results = test_query('*.py config', 'Should return .py files matching "config"')
-- Verify
for _, r in ipairs(py_config_results) do
  local path = r.relative_path or r.path
  if not path:match('%.py$') then
    print('  ERROR: Non-.py file: ' .. path)
  end
end

-- Test 5: Path segment constraint
test_query('/src/', 'Should return files in src directories')

-- Test 6: Trailing slash path segment
test_query('models/', 'Should return files in models directories')

-- Test 7: Multiple fuzzy parts
test_query('user auth handler', 'Should match files containing all three terms')

-- Test 8: Glob pattern
local glob_results = test_query('*.{py,rs}', 'Should return .py and .rs files')
for _, r in ipairs(glob_results) do
  local path = r.relative_path or r.path
  if not (path:match('%.py$') or path:match('%.rs$')) then
    print('  ERROR: Non-.py/.rs file: ' .. path)
  end
end

-- Test 9: Mixed constraints and fuzzy
test_query('*.ts /components/ button', 'Should return .ts files in components with "button"')

-- Test 10: Short single word
test_query('main', 'Should match files with "main" in path')

print('\n========== TESTS COMPLETE ==========\n')
