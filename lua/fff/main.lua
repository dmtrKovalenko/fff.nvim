local fuzzy = require('fff.fuzzy')
local git_utils = require('fff.git_utils')
if not fuzzy then error('Failed to load fff.fuzzy module. Ensure the Rust backend is compiled and available.') end

local M = {}
M.config = {}
M.state = { initialized = false }

--- Setup the file picker with the given configuration
--- @param config table Configuration options
function M.setup(config)
  local default_config = {
    base_path = vim.fn.getcwd(),
    max_results = 100,
    prompt = '🪿 ', -- Input prompt symbol
    title = 'FFF Files', -- Window title
    width = 0.8,
    height = 0.8,
    preview = {
      enabled = true,
      width = 0.5,
      max_lines = 100,
      max_size = 1024 * 1024, -- 1MB
    },
    keymaps = {
      close = '<Esc>',
      select = '<CR>',
      select_split = '<C-s>',
      select_vsplit = '<C-v>',
      select_tab = '<C-t>',
      move_up = { '<Up>', '<C-p>' },
      move_down = { '<Down>', '<C-n>' },
      preview_scroll_up = '<C-u>',
      preview_scroll_down = '<C-d>',
    },
    hl = {
      border = 'FloatBorder',
      normal = 'Normal',
      cursor = 'CursorLine',
      matched = 'IncSearch',
      title = 'Title',
      prompt = 'Question',
      active_file = 'Visual',
      frecency = 'Number',
      debug = 'Comment',
    },
    layout = {
      prompt_position = 'top',
      preview_position = 'right',
      preview_width = 0.4,
      height = 0.8,
      width = 0.8,
    },
    frecency = {
      enabled = true,
      db_path = vim.fn.stdpath('cache') .. '/fff_nvim',
    },
    debug = {
      enabled = false,
      show_scores = false,
    },
    logging = {
      enabled = true,
      log_file = vim.fn.stdpath('log') .. '/fff.log',
      log_level = 'info',
    },
    ui = {
      wrap_paths = true,
      wrap_indent = 2,
      max_path_width = 80,
    },
    image_preview = {
      enabled = true,
      max_width = 80,
      max_height = 24,
    },
    icons = {
      enabled = true,
    },
    ui_enabled = true,
  }

  local merged_config = vim.tbl_deep_extend('force', default_config, config or {})
  M.config = merged_config

  local db_path = merged_config.frecency.db_path or (vim.fn.stdpath('cache') .. '/fff_nvim')
  local ok, result = pcall(fuzzy.init_db, db_path, true)
  if not ok then vim.notify('Failed to initialize frecency database: ' .. result, vim.log.levels.WARN) end

  ok, result = pcall(fuzzy.init_file_picker, merged_config.base_path)
  if not ok then
    vim.notify('Failed to initialize file picker: ' .. result, vim.log.levels.ERROR)
    return false
  end

  M.state.initialized = true
  M.config = merged_config

  M.setup_commands()
  if merged_config.frecency.enabled then M.setup_global_file_tracking() end
  git_utils.setup_highlights()

  if merged_config.logging.enabled then
    local log_success, log_error =
      pcall(fuzzy.init_tracing, merged_config.logging.log_file, merged_config.logging.log_level)
    if log_success then
      M.log_file_path = log_error
    else
      vim.notify('Failed to initialize logging: ' .. (tostring(log_error) or 'unknown error'), vim.log.levels.WARN)
    end
  end

  return true
end

function M.setup_global_file_tracking()
  local group = vim.api.nvim_create_augroup('fff_file_tracking', { clear = true })

  vim.api.nvim_create_autocmd({ 'BufRead', 'BufNewFile' }, {
    group = group,
    callback = function(args)
      local file_path = args.file

      if file_path and file_path ~= '' and not vim.startswith(file_path, 'term://') then
        -- never block the UI
        vim.schedule(function()
          local stat = vim.loop.fs_stat(file_path)
          if stat and stat.type == 'file' then
            local fuzzy = require('fff.fuzzy')
            local relative_path = vim.fn.fnamemodify(file_path, ':.')
            pcall(fuzzy.access_file, relative_path)
          end
        end)
      end
    end,
    desc = 'Track file access for FFF frecency',
  })
end

function M.setup_commands()
  vim.api.nvim_create_user_command('FFFFind', function(opts)
    if opts.args and opts.args ~= '' then
      -- If argument looks like a directory, use it as base path
      if vim.fn.isdirectory(opts.args) == 1 then
        M.find_files_in_dir(opts.args)
      else
        -- Otherwise treat as search query
        M.search_and_show(opts.args)
      end
    else
      M.find_files()
    end
  end, {
    nargs = '?',
    omplete = function(arg_lead, cmd_line, cursor_pos)
      local dirs = vim.fn.glob(arg_lead .. '*', false, true)
      local results = {}
      for _, dir in ipairs(dirs) do
        if vim.fn.isdirectory(dir) == 1 then table.insert(results, dir) end
      end
      return results
    end,
    desc = 'Find files with FFF (use directory path or search query)',
  })

  vim.api.nvim_create_user_command('FFFScan', function() M.scan_files() end, {
    desc = 'Scan files for FFF',
  })

  vim.api.nvim_create_user_command('FFFRefreshGit', function() M.refresh_git_status() end, {
    desc = 'Manually refresh git status for all files',
  })

  vim.api.nvim_create_user_command('FFFClearCache', function(opts) M.clear_cache(opts.args) end, {
    nargs = '?',
    complete = function() return { 'all', 'frecency', 'files' } end,
    desc = 'Clear FFF caches (all|frecency|files)',
  })

  vim.api.nvim_create_user_command('FFFHealth', function() M.health_check() end, {
    desc = 'Check FFF health',
  })

  vim.api.nvim_create_user_command('FFFDebug', function(opts)
    if opts.args == 'toggle' or opts.args == '' then
      M.config.debug.show_scores = not M.config.debug.show_scores
      local status = M.config.debug.show_scores and 'enabled' or 'disabled'
      vim.notify('FFF debug scores ' .. status, vim.log.levels.INFO)
    elseif opts.args == 'on' then
      M.config.debug.show_scores = true
      vim.notify('FFF debug scores enabled', vim.log.levels.INFO)
    elseif opts.args == 'off' then
      M.config.debug.show_scores = false
      vim.notify('FFF debug scores disabled', vim.log.levels.INFO)
    else
      vim.notify('Usage: :FFFDebug [on|off|toggle]', vim.log.levels.ERROR)
    end
  end, {
    nargs = '?',
    complete = function() return { 'on', 'off', 'toggle' } end,
    desc = 'Toggle FFF debug scores display',
  })

  vim.api.nvim_create_user_command('FFFOpenLog', function()
    if M.log_file_path then
      vim.cmd('tabnew ' .. vim.fn.fnameescape(M.log_file_path))
    elseif M.config and M.config.logging and M.config.logging.log_file then
      -- Fallback to the configured log file path even if tracing wasn't initialized
      vim.cmd('tabnew ' .. vim.fn.fnameescape(M.config.logging.log_file))
    else
      vim.notify('Log file path not available', vim.log.levels.ERROR)
    end
  end, {
    desc = 'Open FFF log file in new tab',
  })
end

--- Find files in current directory
function M.find_files()
  local picker_ok, picker_ui = pcall(require, 'fff.picker_ui')
  if picker_ok then
    picker_ui.open()
  else
    vim.notify('Failed to load picker UI', vim.log.levels.ERROR)
  end
end

--- Find files in specific directory
--- @param dir string Directory path
function M.find_files_in_dir(dir)
  local picker_ok, picker_ui = pcall(require, 'fff.picker_ui')
  if picker_ok then
    picker_ui.open({ cwd = dir })
  else
    vim.notify('Failed to load picker UI', vim.log.levels.ERROR)
  end
end

--- Find files in git repository root
function M.find_in_git_root()
  -- Check if we're in a git repo first
  local git_root = vim.fn.system('git rev-parse --show-toplevel 2>/dev/null'):gsub('\n', '')
  if vim.v.shell_error ~= 0 then
    vim.notify('Not in a git repository', vim.log.levels.WARN)
    return
  end

  local picker_ok, picker_ui = pcall(require, 'fff.picker_ui')
  if picker_ok then
    picker_ui.open({ title = 'Git Files', cwd = git_root })
  else
    vim.notify('Failed to load picker UI', vim.log.levels.ERROR)
  end
end

--- Find recent files (frecency based)
function M.find_recent()
  local picker_ok, picker_ui = pcall(require, 'fff.picker_ui')
  if picker_ok then
    picker_ui.open({ title = 'Recent Files' })
  else
    vim.notify('Failed to load picker UI', vim.log.levels.ERROR)
  end
end

--- Toggle file picker
function M.toggle() M.find_files() end

--- Scan files
function M.scan_files()
  local fuzzy = require('fff.fuzzy')
  local ok = pcall(fuzzy.scan_files)
  if ok then
    local cached_files = pcall(fuzzy.get_cached_files) and fuzzy.get_cached_files() or {}
    print('Triggered file scan (currently ' .. #cached_files .. ' files cached)')
  else
    vim.notify('Failed to scan files', vim.log.levels.ERROR)
  end
end

--- Refresh git status for all cached files
function M.refresh_git_status()
  local fuzzy = require('fff.fuzzy')
  local ok, files = pcall(fuzzy.refresh_git_status)
  if ok then
    print('Refreshed git status for ' .. #files .. ' files')
  else
    vim.notify('Failed to refresh git status', vim.log.levels.ERROR)
  end
end

--- Search files programmatically
--- @param query string Search query
--- @param max_results number Maximum number of results
--- @return table List of matching files
function M.search(query, max_results)
  local fuzzy = require('fff.fuzzy')
  max_results = max_results or M.config.max_results
  local ok, search_result = pcall(fuzzy.fuzzy_search_files, query, max_results, nil, nil)
  if ok and search_result.items then return search_result.items end
  return {}
end

--- Search and show results in a nice format
--- @param query string Search query
function M.search_and_show(query)
  if not query or query == '' then
    M.find_files()
    return
  end

  local results = M.search(query, 20)

  if #results == 0 then
    print('🔍 No files found matching "' .. query .. '"')
    return
  end

  -- Filter out directories (should already be done by Rust, but just in case)
  local files = {}
  for _, item in ipairs(results) do
    if not item.is_dir then table.insert(files, item) end
  end

  if #files == 0 then
    print('🔍 No files found matching "' .. query .. '"')
    return
  end

  print('🔍 Found ' .. #files .. ' files matching "' .. query .. '":')

  for i, file in ipairs(files) do
    if i <= 15 then
      local icon = file.extension ~= '' and '.' .. file.extension or '📄'
      local frecency = file.frecency_score > 0 and ' ⭐' .. file.frecency_score or ''
      print('  ' .. i .. '. ' .. icon .. ' ' .. file.relative_path .. frecency)
    end
  end

  if #files > 15 then print('  ... and ' .. (#files - 15) .. ' more files') end

  print('Use :FFFFind to browse all files')
end

--- Get file preview
--- @param file_path string Path to the file
--- @return string|nil File content or nil if failed
function M.get_preview(file_path)
  local preview = require('fff.file_picker.preview')
  local temp_buf = vim.api.nvim_create_buf(false, true)
  local success = preview.preview(file_path, temp_buf)
  if not success then
    vim.api.nvim_buf_delete(temp_buf, { force = true })
    return nil
  end
  local lines = vim.api.nvim_buf_get_lines(temp_buf, 0, -1, false)
  vim.api.nvim_buf_delete(temp_buf, { force = true })
  return table.concat(lines, '\n')
end

function M.debug_file_ordering()
  print('FFF Debug File Ordering')
  print('=======================')

  if not M.is_initialized() then
    print('File picker not initialized. Run :FFFScan first.')
    return
  end

  local picker = M.picker or M.core
  print('Getting top 10 files with debug info...')

  -- Enable debug mode temporarily
  local old_debug = M.config.debug.show_scores
  M.config.debug.show_scores = true

  -- Search with empty query to get default ordering
  local files = picker.search_files('', 10)

  print('🏆 TOP FILES (in order they appear):')
  print('=' .. string.rep('=', 70))

  for i, file in ipairs(files) do
    local frecency_stars = ''
    if file.frecency_score > 0 then frecency_stars = ' ⭐' .. file.frecency_score end

    -- Extract directory information
    local dir = vim.fn.fnamemodify(file.relative_path, ':h')
    local filename = vim.fn.fnamemodify(file.relative_path, ':t')
    local dir_display = (dir == '.' or dir == '') and 'root' or dir

    -- Get score information for this file
    local score = picker.get_file_score(i)

    print(string.format('%2d. %s%s', i, filename, frecency_stars))
    print(string.format('    Path: %s/', dir_display))
    print(string.format('    Debug: %s', score and score.match_type or 'no debug info'))
    if score then
      print(
        string.format(
          '    Total Score: %d (base=%d, name_bonus=%d, special_bonus=%d, frec=%d, dist=%d)',
          score.total,
          score.base_score,
          score.filename_bonus,
          score.special_filename_bonus,
          score.frecency_boost,
          score.distance_penalty
        )
      )
    else
      print('    Total Score: N/A (no score data)')
    end

    local now = os.time()
    local age_hours = math.floor((now - file.modified) / 3600)
    local age_days = math.floor(age_hours / 24)
    print(string.format('    Age: %d hours (%d days) since last modified', age_hours, age_days))
    print('')
  end

  print('💡 EXPLANATION:')
  print('• Files are sorted by FRECENCY first (⭐ score), then by modification time')
  print('• Frecency combines how often AND how recently you accessed files')
  print('• The file at #1 has either:')
  print('  - Highest frecency score, OR')
  print('  - Same frecency as others but most recent modification')

  M.config.debug.show_scores = old_debug
end

function M.health_check()
  local health = {
    ok = true,
    messages = {},
  }

  local errors = check_dependencies()
  if #errors > 0 then
    health.ok = false
    for _, error in ipairs(errors) do
      table.insert(health.messages, error)
    end
  end

  local ui_errors = check_ui_dependencies()
  if #ui_errors > 0 then
    table.insert(health.messages, 'UI not available: ' .. table.concat(ui_errors, ', '))
  else
    table.insert(health.messages, '✓ UI available')
  end

  if not M.is_initialized() then
    health.ok = false
    table.insert(health.messages, 'File picker not initialized')
  else
    table.insert(health.messages, '✓ File picker initialized')
  end

  local optional_deps = {
    { cmd = 'git', desc = 'Git integration' },
    { cmd = 'chafa', desc = 'Terminal graphics for image preview' },
    { cmd = 'img2txt', desc = 'ASCII art for image preview' },
    { cmd = 'viu', desc = 'Terminal images for image preview' },
  }

  for _, dep in ipairs(optional_deps) do
    if vim.fn.executable(dep.cmd) == 0 then
      table.insert(health.messages, string.format('Optional: %s not found (%s)', dep.cmd, dep.desc))
    else
      table.insert(health.messages, string.format('✓ %s found', dep.cmd))
    end
  end

  if health.ok then
    vim.notify('FFF health check passed ✓', vim.log.levels.INFO)
  else
    vim.notify('FFF health check failed ✗', vim.log.levels.ERROR)
  end

  for _, message in ipairs(health.messages) do
    local level = message:match('^✓') and vim.log.levels.INFO
      or message:match('^Optional:') and vim.log.levels.WARN
      or vim.log.levels.ERROR
    vim.notify(message, level)
  end

  return health
end

function M.get_status()
  local status = 'No files indexed'

  local ok, cached_files = pcall(fuzzy.get_cached_files)
  if ok and cached_files and #cached_files > 0 then status = string.format('%d files indexed', #cached_files) end

  if M.config and M.config.frecency and M.config.frecency.enabled then
    status = status .. ' • Frecency tracking enabled'
  end

  return status
end

function M.is_initialized() return M.state and M.state.initialized or false end

return M
