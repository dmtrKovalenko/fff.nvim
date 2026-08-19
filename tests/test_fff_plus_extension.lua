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
  assert(type(git_files.create) == 'function', 'git_files.create should build a shared picker adapter')
  assert(type(git_files.get_git_root) == 'function', 'git_files.get_git_root should be a function')
  print('✓ fff_plus picker modules load correctly')
end

local function test_git_shared_picker_adapter()
  print('Testing Git shared picker adapter...')
  local git_files = require('fff_plus.pickers.git_files')
  local original_root = git_files.get_git_root
  local original_status = git_files.get_git_status_files
  git_files.get_git_root = function(cwd, done)
    assert(cwd == '/repo/work')
    done('/repo')
    return { kill = function() end }
  end
  git_files.get_git_status_files = function(root, done)
    assert(root == '/repo')
    done({
      {
        name = 'README.md',
        path = '/repo/README.md',
        relative_path = 'README.md',
        git_status = 'modified',
      },
    })
    return { kill = function() end }
  end

  local instance = git_files.create({ cwd = '/repo/work', enter = false, preview = { enabled = false } })
  instance:refresh()
  git_files.get_git_root = original_root
  git_files.get_git_status_files = original_status

  assert(instance.spec.name == 'git_files', 'Git files should use the shared picker interface')
  assert(instance.git_root == '/repo' and instance:count() == 1, 'Git adapter should resolve and load asynchronously')
  assert(instance:format(instance:current()).text:find('README.md', 1, true), 'Git adapter should format source items')
  instance:close(false)
  print('✓ Git files uses the shared picker interface')
end

local function test_colors_shared_picker_adapter()
  print('Testing colors shared picker adapter...')
  local colors = require('fff_plus.pickers.colors')
  local original_items = colors.get_colorscheme_items
  colors.get_colorscheme_items = function()
    return {
      { name = 'default', current = true },
      { name = 'habamax', current = false },
    }
  end

  local instance = colors.create({ enter = false })
  instance:refresh()
  colors.get_colorscheme_items = original_items

  assert(instance.spec.name == 'colors', 'colors should be a shared picker adapter')
  assert(instance:count() == 2, 'colors adapter should supply colorscheme items')
  assert(instance:format(instance:current()).text:find('default', 1, true), 'colors adapter should format its items')
  instance:close(false)
  print('✓ colors uses the shared picker interface')
end

local function test_buffers_shared_picker_adapter()
  print('Testing buffers shared picker adapter...')
  local buffers = require('fff_plus.pickers.buffers')
  local instance = buffers.create({ enter = false, preview = { enabled = false } })
  instance:refresh()

  assert(instance.spec.name == 'buffers', 'buffers should be a shared picker adapter')
  assert(instance:count() >= 1, 'buffers adapter should supply listed buffers')
  assert(instance:format(instance:current()).text, 'buffers adapter should return structured formatting')
  instance:close(false)
  print('✓ buffers uses the shared picker interface')
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
  local git_matches = git_files.filter_files({ { relative_path = 'lua/fff_plus/matcher.lua' } }, 'fpm')
  assert(#git_matches == 1, 'Git picker should use fuzzy subsequence matching')
  print('✓ fuzzy matcher ranks subsequence matches')
end

local function test_viewport_calculation()
  print('Testing picker viewport...')
  local viewport = require('fff_plus.viewport')

  local first_page = viewport.calculate(3, 1, 5, 'bottom')
  assert(first_page.first == 1 and first_page.last == 3)
  assert(first_page.padding == 2 and first_page.cursor_line == 5)
  assert(first_page.reverse == true, 'bottom prompt should render best matches nearest the prompt')

  local scrolled = viewport.calculate(20, 12, 5, 'bottom')
  assert(scrolled.first == 8 and scrolled.last == 12)
  assert(scrolled.padding == 0 and scrolled.cursor_line == 1)

  local top = viewport.calculate(20, 7, 5, 'top')
  assert(top.first == 3 and top.last == 7)
  assert(top.padding == 0 and top.cursor_line == 5)

  assert(viewport.move(1, 20, 'up', 'bottom') == 2)
  assert(viewport.move(2, 20, 'down', 'bottom') == 1)
  assert(viewport.move(2, 20, 'up', 'top') == 1)
  assert(viewport.move(2, 20, 'down', 'top') == 3)
  print('✓ picker viewport keeps the logical cursor visible')
end

local function test_picker_layout()
  print('Testing picker layout...')
  local layout = require('fff_plus.layout')

  local floating = layout.frame(120, 40, { width = 0.8, height = 0.8 }, false)
  assert(floating.width == 96 and floating.height == 32)
  assert(floating.col == 12 and floating.row == 4)

  local fullscreen = layout.frame(120, 40, {}, true)
  assert(fullscreen.width == 116 and fullscreen.height == 36)
  assert(fullscreen.col == 1 and fullscreen.row == 0)
  print('✓ picker layout resolves floating and fullscreen frames')
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

  local original_run = git_source.run
  local captured = {}
  git_source.run = function(command, opts, done)
    table.insert(captured, { command = command, opts = opts })
    local output = command[2] == 'ls-files' and 'README.md\0'
      or (command[2] == 'status' and ' M README.md\0' or 'diff --git a/file b/file\n')
    done({ ok = true, code = 0, stdout = output, stderr = '' })
    return { kill = function() end }
  end

  local async_tracked
  git_source.tracked('/repo', function(items) async_tracked = items end)
  assert(async_tracked[1] == 'README.md', 'tracked should parse asynchronous process output')

  local async_status
  git_source.status('/repo', function(items) async_status = items end)
  assert(async_status[1].git_status == 'modified', 'status should parse asynchronous process output')

  local diff
  git_source.diff('/repo', 'lua/file with spaces.lua', function(value) diff = value end)
  local staged
  git_source.stage('/repo', { 'lua/file with spaces.lua' }, function(ok) staged = ok end)
  git_source.run = original_run
  assert(diff:find('diff %-%-git'), 'Git diff should return command output')
  assert(captured[3].command[2] == 'diff' and captured[3].command[#captured[3].command] == 'lua/file with spaces.lua')
  assert(captured[3].opts.cwd == '/repo', 'Git commands should pass the repository as cwd')
  assert(staged and captured[4].command[2] == 'add' and captured[4].command[3] == '--')
  assert(captured[4].command[4] == 'lua/file with spaces.lua', 'Git mutations should preserve raw path argv')
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

  local current_buf = vim.api.nvim_get_current_buf()
  vim.api.nvim_buf_set_lines(current_buf, 0, -1, false, { 'anchor' })
  vim.api.nvim_win_set_cursor(0, { 1, 0 })
  selection.put({ { path = 'a.lua' }, { path = 'b.lua' } }, function(item) return item.path end)
  local pasted = vim.api.nvim_buf_get_lines(current_buf, 0, -1, false)
  assert(pasted[2] == 'a.lua' and pasted[3] == 'b.lua', 'paste should insert selected paths')
  print('✓ picker selection preserves item order and current-item fallback')
end

local function test_picker_actions()
  print('Testing picker actions...')
  local buffers = require('fff_plus.pickers.buffers')
  local bufnr = vim.api.nvim_get_current_buf()
  local buffer_item = {
    bufnr = bufnr,
    line = 1,
    display_name = 'current buffer',
    path = vim.api.nvim_buf_get_name(bufnr),
  }

  local buffer_picker = buffers.create({ preview = { enabled = false }, jump_to_existing = true })
  buffer_picker.items = { buffer_item }
  buffer_picker.filtered_items = { buffer_item }
  buffer_picker.selected_keys = { [bufnr] = true }
  buffers.state = buffer_picker
  assert(buffers.find_existing_window(bufnr, buffer_picker) == vim.api.nvim_get_current_win())

  buffer_picker:action('qflist')
  local buffer_qf = vim.fn.getqflist({ title = 1, items = 1 })
  assert(buffer_qf.title == 'FFF+ Buffers' and #buffer_qf.items == 1)
  vim.cmd('cclose')

  local git_files = require('fff_plus.pickers.git_files')
  local git_source = require('fff_plus.git_source')
  local original_diff = git_source.diff
  git_source.diff = function(root, path, done)
    assert(root == '/repo' and path == 'README.md')
    done('diff --git a/README.md b/README.md')
    return { kill = function() end }
  end
  local git_picker = git_files.create({ source = 'status', preview = { enabled = true } })
  git_picker.git_root = '/repo'
  local diff_preview
  local diff_job = git_picker.spec.preview(
    git_picker,
    { path = '/repo/README.md', relative_path = 'README.md', git_status = 'modified' },
    function(value) diff_preview = value end
  )
  git_source.diff = original_diff
  assert(type(diff_job.kill) == 'function', 'Git diff preview should return a cancellable job')
  assert(diff_preview.filetype == 'diff' and diff_preview.lines[1]:find('diff %-%-git'))

  local original_stage = git_source.stage
  local staged
  git_source.stage = function(root, paths, done)
    staged = { root = root, paths = paths }
    done(true, { ok = true })
    return { kill = function() end }
  end
  git_picker.items = {
    { path = '/repo/README.md', relative_path = 'README.md', git_status = 'modified' },
  }
  git_picker.filtered_items = git_picker.items
  git_picker.refresh = function() git_picker.refreshed = true end
  git_picker:action('stage')
  git_source.stage = original_stage
  assert(staged.root == '/repo' and staged.paths[1] == 'README.md', 'stage should pass selected paths as argv')
  assert(git_picker.refreshed == true, 'successful Git actions should refresh the picker')

  local original_restore = git_source.restore
  local restore_calls = 0
  git_source.restore = function(_, _, done)
    restore_calls = restore_calls + 1
    done(true, { ok = true })
    return { kill = function() end }
  end
  git_picker.opts.confirm_restore = function() return 2 end
  git_picker:action('restore')
  assert(restore_calls == 0, 'cancelled restore confirmation should not mutate the worktree')
  git_picker.opts.confirm_restore = function() return 1 end
  git_picker:action('restore')
  git_source.restore = original_restore
  assert(restore_calls == 1, 'confirmed restore should mutate the worktree once')
  git_picker:close(false)
  print('✓ picker actions populate quickfix, jump windows, async previews, and Git mutations')
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
  assert(type(require('fff_plus').smart) == 'function', 'smart API should exist')
  assert(type(require('fff_plus').lines) == 'function', 'lines API should exist')
  assert(type(require('fff_plus').loaded_lines) == 'function', 'loaded_lines API should exist')
  assert(type(require('fff_plus').diagnostics) == 'function', 'diagnostics API should exist')
  assert(type(require('fff_plus').buffer_diagnostics) == 'function', 'buffer_diagnostics API should exist')
  assert(type(require('fff_plus').resume) == 'function', 'resume API should exist')

  local commands = vim.api.nvim_get_commands({ builtin = false })
  assert(commands.FFFPlusBuffers.bang, 'FFFPlusBuffers should support fullscreen bang')
  assert(commands.FFFPlusColors.bang, 'FFFPlusColors should support fullscreen bang')
  assert(commands.FFFPlusGitFiles.bang, 'FFFPlusGitFiles should support fullscreen bang')
  assert(commands.FFFPlusGitStatus.bang, 'FFFPlusGitStatus should support fullscreen bang')
  assert(commands.FFFPlusSmart.bang, 'FFFPlusSmart should support fullscreen bang')
  assert(commands.FFFPlusLines.bang, 'FFFPlusLines should support fullscreen bang')
  assert(commands.FFFPlusLoadedLines.bang, 'FFFPlusLoadedLines should support fullscreen bang')
  assert(commands.FFFPlusDiagnostics.bang, 'FFFPlusDiagnostics should support fullscreen bang')
  assert(commands.FFFPlusBufferDiagnostics.bang, 'FFFPlusBufferDiagnostics should support fullscreen bang')
  assert(commands.FFFPlusResume.bang, 'FFFPlusResume should support fullscreen bang')

  require('fff_plus').setup({ legacy_commands = true })
  local plus = require('fff_plus')
  local original_open = plus.open
  local opened
  plus.open = function(name, opts)
    opened = { name = name, opts = opts or {} }
    return true
  end

  vim.cmd('GFiles')
  assert(opened.name == 'git_files' and opened.opts.source == 'tracked', ':GFiles should dispatch tracked files')
  vim.cmd('FFFPlusGitStatus!')
  assert(opened.name == 'git_files' and opened.opts.fullscreen == true, 'bang should propagate fullscreen')
  plus.open = original_open
  print('✓ fff_plus commands register correctly')
end

local function run_tests()
  print('\n=== Running fff_plus extension tests ===\n')

  local tests = {
    test_extension_module_loads,
    test_picker_modules_load,
    test_colors_shared_picker_adapter,
    test_buffers_shared_picker_adapter,
    test_git_shared_picker_adapter,
    test_fuzzy_matcher,
    test_viewport_calculation,
    test_picker_layout,
    test_git_sources,
    test_picker_selection,
    test_picker_actions,
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

if not run_tests() then error('fff-plus extension tests failed') end
