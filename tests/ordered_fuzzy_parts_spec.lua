---@diagnostic disable: undefined-field, missing-fields
-- Config test for the opt-in `file_picker.ordered_fuzzy_parts` setting:
-- space-separated fuzzy query parts must match in the order they were typed,
-- as a single fuzzy subsequence, instead of each part matching independently
-- anywhere in the candidate. Default (off) behavior must stay unchanged.

local fff = require('fff')
local fff_rust = require('fff.rust')
local file_picker = require('fff.file_picker')

--- Normalise a path so comparisons work across symlinked temp dirs
--- (e.g. macOS `/tmp` -> `/private/tmp`). Mirrors picker_dir_resolution_spec.lua.
--- @param p string
--- @return string
local function norm(p)
  local rp = vim.uv.fs_realpath(p) or vim.fn.fnamemodify(vim.fn.resolve(p), ':p')
  local n = vim.fs.normalize(rp)
  n = n:gsub('/$', '')
  if vim.fn.has('win32') == 1 then n = n:lower() end
  return n
end

--- `change_indexing_directory` swaps the picker on a background thread, so the
--- `FILE_PICKER` global may still point at the *old* picker for a moment —
--- mirrors the same wait helper in picker_dir_resolution_spec.lua.
local function wait_for_reindex(expected_dir, timeout_ms)
  local expected = norm(expected_dir)
  local deadline = vim.uv.hrtime() + timeout_ms * 1e6
  while vim.uv.hrtime() < deadline do
    local ok, health = pcall(fff_rust.health_check, expected)
    if ok and health and health.file_picker and health.file_picker.base_path then
      if norm(health.file_picker.base_path) == expected then return true end
    end
    vim.wait(20, function() return false end)
  end
  return false
end

local function find_result_by_relative_path(items, relative_path)
  for _, item in ipairs(items) do
    if item.relative_path == relative_path or item.relative_path:gsub('\\', '/') == relative_path then
      return item
    end
  end
  return nil
end

describe('file_picker.ordered_fuzzy_parts', function()
  local sandbox_root

  before_each(function()
    sandbox_root = vim.fn.tempname()
    vim.fn.mkdir(sandbox_root .. '/src/handler', 'p')
    vim.fn.mkdir(sandbox_root .. '/auth/src', 'p')

    -- "handler auth" appears in order: src/handler/auth.lua
    local fd = assert(io.open(sandbox_root .. '/src/handler/auth.lua', 'w'))
    fd:write('return {}\n')
    fd:close()

    -- "handler auth" is reversed: auth/src/handler.lua
    fd = assert(io.open(sandbox_root .. '/auth/src/handler.lua', 'w'))
    fd:write('return {}\n')
    fd:close()

    vim.g.fff = {}
    file_picker.setup()

    -- `require('fff.file_picker')` eagerly calls `ensure_initialized()`,
    -- which may already have scanned the default cwd by now. Explicitly
    -- switch to the sandbox and wait for the swap, rather than assuming
    -- the first init already targeted it.
    assert.is_true(require('fff.core').change_indexing_directory(sandbox_root))
    assert.is_true(wait_for_reindex(sandbox_root, 10000), 'reindex to sandbox did not complete')
    fff_rust.wait_for_initial_scan(30000)
  end)

  after_each(function()
    pcall(fff_rust.stop_background_monitor)
    pcall(fff_rust.cleanup_file_picker)
    vim.g.fff = nil
    if sandbox_root then vim.fn.delete(sandbox_root, 'rf') end
  end)

  it('defaults to off: matches both in-order and reversed paths', function()
    local result = fff.file_search('handler auth')

    assert.is_true(#result.items >= 2, 'expected both files to match with the default (unordered) matcher')
    assert.is_not_nil(find_result_by_relative_path(result.items, 'src/handler/auth.lua'))
    assert.is_not_nil(find_result_by_relative_path(result.items, 'auth/src/handler.lua'))
  end)

  it('when enabled via opts, only matches the in-order path', function()
    local result = fff.file_search('handler auth', { ordered_fuzzy_parts = true })

    assert.are.equal(1, #result.items, 'ordered mode should only match the in-order path')
    assert.is_not_nil(find_result_by_relative_path(result.items, 'src/handler/auth.lua'))
    assert.is_nil(find_result_by_relative_path(result.items, 'auth/src/handler.lua'))
  end)

  it('repeated whitespace between parts is ignored under ordered mode', function()
    local collapsed = fff.file_search('handler auth', { ordered_fuzzy_parts = true })
    local spaced = fff.file_search('handler   auth', { ordered_fuzzy_parts = true })

    assert.are.equal(1, #collapsed.items)
    assert.are.equal(#collapsed.items, #spaced.items)
    assert.are.equal(collapsed.items[1].relative_path, spaced.items[1].relative_path)
  end)

  it('pagination total_matched agrees with the returned items under ordered mode', function()
    local result = fff.file_search('handler auth', { ordered_fuzzy_parts = true })

    assert.are.equal(1, result.total_matched)
    assert.are.equal(result.total_matched, #result.items)
  end)

  it('directory search mode also respects ordered_fuzzy_parts', function()
    local off_result = fff.file_search('handler auth', { mode = 'directories' })
    local on_result = fff.file_search('handler auth', { mode = 'directories', ordered_fuzzy_parts = true })

    assert.is_true(#off_result.items >= #on_result.items)
  end)
end)
