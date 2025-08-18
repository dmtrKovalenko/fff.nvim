local fuzzy = require('fff.fuzzy')

if not fuzzy then error('Failed to load fff.fuzzy module. Ensure the Rust backend is compiled and available.') end

local M = {}

---@class fff.core.State
local state = {
  ---@type boolean
  initialized = false
}

---@param config table
local function setup_global_autocmds(config)
  local group = vim.api.nvim_create_augroup('fff_file_tracking', { clear = true })

  if config.frecency.enabled then
    vim.api.nvim_create_autocmd({ 'BufReadPost' }, {
      group = group,
      desc = 'Track file access for FFF frecency',
      callback = function(args)
        local file_path = args.file
        if not (file_path and file_path ~= '' and not vim.startswith(file_path, 'term://')) then return end

        vim.uv.fs_stat(file_path, function(err, stat)
          if err or not stat then return end

          vim.uv.fs_realpath(file_path, function(rp_err, real_path)
            if rp_err or not real_path then return end
            local ok, track_err = pcall(fuzzy.track_access, real_path)

            if not ok then
              vim.notify('FFF: Failed to track file access: ' .. tostring(track_err), vim.log.levels.ERROR)
            end
          end)
        end)
      end,
    })
  end

  -- make sure that this won't work correctly if autochdir plugins are enabled
  -- using a pure :cd command but will work using lua api or :e command
  vim.api.nvim_create_autocmd('DirChanged', {
    group = group,
    callback = function()
      local new_cwd = vim.v.event.cwd
      if M.is_initialized() and new_cwd and new_cwd ~= M.config.base_path then
        vim.schedule(function()
          local ok, err = pcall(M.change_indexing_directory, new_cwd)
          if not ok then
            vim.notify('FFF: Failed to change indexing directory: ' .. tostring(err), vim.log.levels.ERROR)
          else
            M.config.base_path = new_cwd
          end
        end)
      end
    end,
    desc = 'Automatically sync FFF directory changes',
  })

  vim.api.nvim_create_autocmd('VimLeavePre', {
    group = group,
    callback = function() pcall(fuzzy.cleanup_file_picker) end,
    desc = 'Cleanup FFF background threads on Neovim exit',
  })
end

local function setup_commands()
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

M.ensure_enitialized = function()
  if state.initialized then
    return true
  end

  local config = require('fff.config').get()
  if config.logging.enabled then
    local log_success, log_error =
        pcall(fuzzy.init_tracing, config.logging.log_file, config.logging.log_level)
    if log_success then
      M.log_file_path = log_error
    else
      vim.notify('Failed to initialize logging: ' .. (tostring(log_error) or 'unknown error'), vim.log.levels.WARN)
    end
  end

  local db_path = config.frecency.db_path or (vim.fn.stdpath('cache') .. '/fff_nvim')
  local ok, result = pcall(fuzzy.init_db, db_path, true)
  if not ok then vim.notify('Failed to initialize frecency database: ' .. result, vim.log.levels.WARN) end

  ok, result = pcall(fuzzy.init_file_picker, config.base_path)
  if not ok then
    vim.notify('Failed to initialize file picker: ' .. result, vim.log.levels.ERROR)
    return false
  end

  state.initialized = true

  setup_commands()
  setup_global_autocmds(config)

  local git_utils = require('fff.git_utils')
  git_utils.setup_highlights()

  return true
end

return M
