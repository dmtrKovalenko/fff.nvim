-- Sanity test: open fresh (non-existent) LMDB dbs, run a couple of picker
-- calls, then close. Verifies the health_check returns `healthy = true` on a
-- freshly-initialized tracker — i.e. the GC thread actually ran and flipped
-- the flag out of Pending.
--
-- Run with:
--   nvim -l tests/fresh_db_open_test.lua
--
-- Exit code 0 = success, non-zero = failure (error printed).

local function die(msg)
  io.stderr:write('FAIL: ' .. msg .. '\n')
  os.exit(1)
end

local function ok(msg) print('ok  ' .. msg) end

-- Resolve plugin dir and add to runtimepath. `arg[0]` is the script path
-- under `nvim -l`, while `<sfile>` is not set in that mode.
local script_path = arg and arg[0] or debug.getinfo(1, 'S').source:sub(2)
local plugin_dir = vim.fn.fnamemodify(vim.fn.resolve(script_path), ':h:h')
vim.opt.runtimepath:prepend(plugin_dir)

-- Force brand new db paths so we exercise the fresh-open code path
local tmp_frecency = vim.fn.tempname() .. '_fresh_frec'
local tmp_history = vim.fn.tempname() .. '_fresh_hist'

vim.fn.delete(tmp_frecency, 'rf')
vim.fn.delete(tmp_history, 'rf')

local fff_rust = require('fff.rust')

-- Init dbs at the fresh paths
local init_ok = fff_rust.init_db(tmp_frecency, tmp_history, true)
if not init_ok then die('init_db returned false') end
ok('init_db(fresh paths)')

-- Init the picker rooted at the plugin dir so there's something to scan
local picker_ok = fff_rust.init_file_picker(plugin_dir)
if not picker_ok then die('init_file_picker returned false') end
ok('init_file_picker(plugin_dir)')

fff_rust.wait_for_initial_scan(10000)
ok('initial scan complete')

-- Actually run a search so the picker touches the frecency/query dbs.
-- Signature: (query, max_threads, current_file, combo_boost, min_combo, page_index, page_size)
local results = fff_rust.fuzzy_search_files('lib.rs', 4, nil, 0, nil, nil, nil)
if type(results) ~= 'table' then die('fuzzy_search_files did not return a table') end
ok(string.format('fuzzy_search_files returned %d results', #(results.items or results)))

-- Give the GC thread up to ~2s to flip Pending -> Healthy
---@type any
local health = nil
local deadline = vim.loop.now() + 2000
while vim.loop.now() < deadline do
  health = fff_rust.health_check(plugin_dir)
  local frec = health and health.frecency and health.frecency.db_healthcheck
  local qt = health and health.query_tracker and health.query_tracker.db_healthcheck
  if frec and qt and frec.healthy == true and qt.healthy == true then break end
  vim.wait(50)
end

if not health then die('health_check returned nil') end
ok('health_check returned a table')

local frec = health.frecency and health.frecency.db_healthcheck
local qt = health.query_tracker and health.query_tracker.db_healthcheck

if not frec then die('frecency db_healthcheck missing from health result') end
if not qt then die('query_tracker db_healthcheck missing from health result') end

if frec.healthy ~= true then die('frecency.healthy expected true, got ' .. tostring(frec.healthy)) end
ok('frecency.healthy = true')

if qt.healthy ~= true then die('query_tracker.healthy expected true, got ' .. tostring(qt.healthy)) end
ok('query_tracker.healthy = true')

-- Confirm the db dirs now exist on disk (proves we actually opened them)
if vim.fn.isdirectory(tmp_frecency) ~= 1 then die('frecency dir missing after init: ' .. tmp_frecency) end
if vim.fn.isdirectory(tmp_history) ~= 1 then die('history dir missing after init: ' .. tmp_history) end
ok('db directories exist on disk')

-- Cleanup
pcall(fff_rust.stop_background_monitor)
pcall(fff_rust.cleanup_file_picker)
pcall(fff_rust.destroy_frecency_db)
pcall(fff_rust.destroy_query_db)
vim.fn.delete(tmp_frecency, 'rf')
vim.fn.delete(tmp_history, 'rf')
ok('cleanup complete')

print('\nALL CHECKS PASSED')
os.exit(0)
