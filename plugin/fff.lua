if vim.g.fff_loaded then return end
vim.g.fff_loaded = true

-- Defer indexing until after UIEnter, so as to not block the UI.
-- This is equivalent to lazy.nvim's VeryLazy, but works with all plugin managers.

local init = vim.schedule_wrap(function()
  if vim.v.exiting ~= vim.NIL then return end
  -- PERF: We query the vim.g.fff config to avoid eagerly requiring Lua modules
  local lazy_sync = vim.tbl_get(vim.g, 'fff', 'lazy_sync')
  if lazy_sync == nil or not lazy_sync then require('fff.core').ensure_initialized() end
  if Snacks and pcall(require, 'snacks.picker') then
    -- Users can call Snacks.picker.fff() after this
    Snacks.picker.sources.fff = require('fff.snacks').source
  end
end)

if vim.v.vim_did_enter == 1 then
  init()
else
  vim.api.nvim_create_autocmd('UIEnter', {
    group = vim.api.nvim_create_augroup('fff.main', {}),
    once = true,
    nested = true,
    callback = init,
  })
end

vim.api.nvim_create_user_command('FFFFind', function(opts)
  local fff = require('fff')
  if opts.args and opts.args ~= '' then
    -- If argument looks like a directory, use it as base path
    if vim.fn.isdirectory(opts.args) == 1 then
      fff.find_files_in_dir(opts.args)
    else
      -- Otherwise treat as search query
      fff.search_and_show(opts.args)
    end
  else
    fff.find_files()
  end
end, {
  nargs = '?',
  complete = function(arg_lead)
    -- Complete with directories and common search terms
    local dirs = vim.fn.glob(arg_lead .. '*', false, true)
    local results = {}
    for _, dir in ipairs(dirs) do
      if vim.fn.isdirectory(dir) == 1 then table.insert(results, dir) end
    end
    return results
  end,
  desc = 'Find files with FFF (use directory path or search query)',
})

vim.api.nvim_create_user_command('FFFScan', function() require('fff').scan_files() end, {
  desc = 'Scan files for FFF',
})

vim.api.nvim_create_user_command('FFFRefreshGit', function() require('fff').refresh_git_status() end, {
  desc = 'Manually refresh git status for all files',
})

vim.api.nvim_create_user_command('FFFClearCache', function(opts) require('fff').clear_cache(opts.args) end, {
  nargs = '?',
  complete = function() return { 'all', 'frecency', 'files' } end,
  desc = 'Clear FFF caches (all|frecency|files)',
})

vim.api.nvim_create_user_command('FFFHealth', function() require('fff').health_check() end, {
  desc = 'Check FFF health',
})

vim.api.nvim_create_user_command('FFFDebug', function(opts)
  local config = require('fff.conf').get()
  if opts.args == 'toggle' or opts.args == '' then
    config.debug.show_scores = not config.debug.show_scores
    local status = config.debug.show_scores and 'enabled' or 'disabled'
    vim.notify('FFF debug scores ' .. status, vim.log.levels.INFO)
  elseif opts.args == 'on' then
    config.debug.show_scores = true
    vim.notify('FFF debug scores enabled', vim.log.levels.INFO)
  elseif opts.args == 'off' then
    config.debug.show_scores = false
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
  local fff = require('fff')
  local config = require('fff.conf').get()
  if fff.log_file_path then
    vim.cmd('tabnew ' .. vim.fn.fnameescape(fff.log_file_path))
  elseif config and config.logging and config.logging.log_file then
    -- Fallback to the configured log file path even if tracing wasn't initialized
    vim.cmd('tabnew ' .. vim.fn.fnameescape(config.logging.log_file))
  else
    vim.notify('Log file path not available', vim.log.levels.ERROR)
  end
end, {
  desc = 'Open FFF log file in new tab',
})

vim.api.nvim_create_user_command('FFFSnacks', function()
  if Snacks and pcall(require, 'snacks.picker') then
    Snacks.picker(require('fff.snacks').source)
  else
    vim.notify('Snacks is not loaded', vim.log.levels.ERROR)
  end
end, {
  desc = 'Open FFF in snacks picker',
})
