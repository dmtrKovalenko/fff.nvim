local fuzzy = require('fff.fuzzy')
if not fuzzy then error('Failed to load fff.fuzzy module. Ensure the Rust backend is compiled and available.') end

local M = {}

---@class fff.core.State
local state = {
  ---@type boolean
  initialized = false,
  ---@type boolean
  file_picker_initialized = false,
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
      if state.initialized and new_cwd and new_cwd ~= config.base_path then
        vim.schedule(function()
          local picker = require('fff.main')
          local ok, err = pcall(picker.change_indexing_directory, new_cwd)
          if not ok then
            vim.notify('FFF: Failed to change indexing directory: ' .. tostring(err), vim.log.levels.ERROR)
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

--- @return boolean
M.is_file_picker_initialized = function() return state.file_picker_initialized end

M.ensure_initialized = function()
  if state.initialized then return fuzzy end
  state.initialized = true

  local config = require('fff.conf').get()
  if config.logging.enabled then
    local log_success, log_error = pcall(fuzzy.init_tracing, config.logging.log_file, config.logging.log_level)
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
    return fuzzy
  end

  state.file_picker_initialized = true
  setup_global_autocmds(config)

  local git_utils = require('fff.git_utils')
  git_utils.setup_highlights()

  return fuzzy
end

return M
