---@diagnostic disable: undefined-field, missing-fields
-- Regression test for https://github.com/dmtrKovalenko/fff.nvim/issues/389
-- When find_files_in_dir(dir) runs with dir != neovim's cwd, the Rust indexer
-- reports paths relative to `dir`, but the Lua side used to resolve them
-- against neovim's cwd when calling :edit / preview / quickfix — so files
-- opened as phantom buffers and previews showed "No preview available".

local fff_rust = require('fff.rust')
local picker_ui = require('fff.picker_ui')
local file_picker = require('fff.file_picker')

--- Normalise a path so that comparisons work on every OS.
--- Windows complicates things: Rust may return forward-slash paths while
--- vim.fn.resolve uses backslashes, temp paths may contain 8.3 short names
--- (RUNNER~1), and the filesystem is case-insensitive.
--- vim.uv.fs_realpath expands 8.3 names on Windows (unlike vim.fn.resolve).
--- @param p string
--- @return string
local function norm(p)
  -- fs_realpath is the closest Lua equivalent of Rust's std::fs::canonicalize
  -- and expands 8.3 short names on Windows.
  local rp = vim.uv.fs_realpath(p) or vim.fn.fnamemodify(vim.fn.resolve(p), ':p')
  local n = vim.fs.normalize(rp)
  -- Strip trailing slash for consistent comparison
  n = n:gsub('/$', '')
  -- Case-fold on Windows (drive letters, 8.3 short names, etc.)
  if vim.fn.has('win32') == 1 then n = n:lower() end
  return n
end

--- `change_indexing_directory` swaps the picker on a background thread, so the
--- `FILE_PICKER` global may still point at the *old* picker for a moment —
--- `wait_for_initial_scan` on the old picker returns immediately and the
--- search then runs against the new, still-empty index. Poll `health_check`
--- until `base_path` matches the expected dir before waiting on the scan.
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

local function wait_for_scan(expected_dir, timeout_ms)
  assert.is_true(wait_for_reindex(expected_dir, timeout_ms), 'reindex to ' .. expected_dir .. ' did not complete')
  fff_rust.wait_for_initial_scan(timeout_ms)
end

describe('picker find_files_in_dir path resolution (issue #389)', function()
  local sandbox_root, target_dir, other_cwd, target_filename

  before_each(function()
    sandbox_root = vim.fn.tempname()
    target_dir = sandbox_root .. '/target-dir'
    other_cwd = sandbox_root .. '/other-cwd'
    vim.fn.mkdir(target_dir, 'p')
    vim.fn.mkdir(other_cwd, 'p')

    target_filename = 'issue389_target.lua'
    local fd = assert(io.open(target_dir .. '/' .. target_filename, 'w'))
    fd:write('-- issue #389 regression fixture\nreturn true\n')
    fd:close()

    -- Clear the DirChanged autocmd that a previous test run (e.g. fff_core_spec)
    -- may have installed.  Without this, the :cd below triggers a scheduled
    -- change_indexing_directory(other_cwd) that races with our explicit
    -- change_indexing_directory(target_dir) and overwrites the FILE_PICKER.
    pcall(vim.api.nvim_del_augroup_by_name, 'fff_file_tracking')

    vim.cmd('cd ' .. vim.fn.fnameescape(other_cwd))
    -- Equivalent to require('fff').setup({}) — just seeds vim.g.fff — but
    -- avoids the top-level fff module lookup which plenary's sandboxed
    -- require can miss depending on package.path.
    vim.g.fff = {}
    file_picker.setup()
  end)

  after_each(function()
    pcall(picker_ui.close)
    pcall(fff_rust.stop_background_monitor)
    pcall(fff_rust.cleanup_file_picker)
    if sandbox_root then vim.fn.delete(sandbox_root, 'rf') end
  end)

  it(':edit opens the file inside base_path even when neovim cwd differs', function()
    assert.are_not.equal(norm(target_dir), norm(vim.fn.getcwd()))

    assert.is_true(picker_ui.change_indexing_directory(target_dir))
    wait_for_scan(target_dir, 10000)

    local items = file_picker.search_files('', nil, nil, nil, nil)
    assert.is_true(#items > 0, 'indexer returned no items for target_dir (norm=' .. norm(target_dir) .. ')')

    local target_item
    for _, item in ipairs(items) do
      if item.name == target_filename then
        target_item = item
        break
      end
    end
    assert.is_not_nil(target_item, 'target fixture file missing from results')
    -- On Windows Rust may use backslash separators; compare just the filename.
    local rel = target_item.relative_path
    assert.are.equal(target_filename, rel:match('[^/\\]+$') or rel)
    assert.is_nil(
      vim.uv.fs_stat(target_item.relative_path),
      'relative_path should not resolve against cwd — if it does, the test fixture is wrong'
    )

    -- Drive the same code path that user hits when pressing <CR>: populate
    -- UI state as open_ui_with_state would and invoke select('edit').
    picker_ui.state.active = true
    picker_ui.state.filtered_items = items
    picker_ui.state.cursor = (function()
      for i, item in ipairs(items) do
        if item.name == target_filename then return i end
      end
      return 1
    end)()
    picker_ui.state.query = ''
    picker_ui.state.mode = nil
    picker_ui.state.location = nil
    picker_ui.state.suggestion_source = nil
    picker_ui.state.selected_files = {}
    picker_ui.state.selected_items = {}

    picker_ui.select('edit')

    local bufname = vim.api.nvim_buf_get_name(0)
    assert.is_true(bufname ~= '', 'expected :edit to open a buffer with a non-empty name')

    local stat = vim.uv.fs_stat(bufname)
    assert.is_not_nil(stat, 'opened buffer points at a non-existent file: ' .. bufname)

    -- The opened file must be the fixture inside target_dir, not a phantom
    -- file under cwd.
    local expected = norm(target_dir .. '/' .. target_filename)
    local actual = norm(bufname)
    assert.are.equal(expected, actual)
  end)
end)
