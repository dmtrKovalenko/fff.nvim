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
    vim.api.nvim_create_autocmd({ 'BufEnter' }, {
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
              vim.schedule(
                function() vim.notify('FFF: Failed to track file access: ' .. tostring(track_err), vim.log.levels.ERROR) end
              )
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
      -- Window-local `:lcd` / `:tcd` are per-window — they don't change the
      -- effective project root for the picker, so bail before touching
      -- anything else.
      if vim.v.event.scope == 'window' then return end
      if not state.initialized then return end

      local new_cwd = vim.v.event.cwd
      if not new_cwd or new_cwd == '' then return end

      -- Canonicalize both sides before comparing. `vim.v.event.cwd` is
      -- whatever the caller passed to `:cd` (often unexpanded, sometimes
      -- containing `~` or symlinks), while `config.base_path` is the form
      -- the picker was last re-indexed against (post-`expand`). Without
      -- resolving symlinks + ensuring an absolute path, trivially
      -- equivalent paths compare as different (`/private/var/x` vs
      -- `/var/x` on macOS, resolved-vs-unresolved symlinks from LSP root
      -- detection, etc.) and every such mismatch schedules a 450k-file
      -- reindex through the Rust side.
      local function canonicalize(p)
        if not p or p == '' then return p end
        local abs = vim.fn.fnamemodify(vim.fn.expand(p), ':p')
        -- `:p` leaves a trailing slash on directories — strip for
        -- comparison stability.
        abs = abs:gsub('/+$', '')
        local ok, resolved = pcall(vim.fn.resolve, abs)
        return (ok and resolved ~= '') and resolved or abs
      end

      local new_canonical = canonicalize(new_cwd)
      local base_canonical = canonicalize(config.base_path)
      if new_canonical == base_canonical then return end

      vim.schedule(function()
        -- Delay require to avoid circular dependency: core -> main -> picker_ui -> file_picker -> core
        local ok, picker = pcall(require, 'fff.main')
        if not ok then
          vim.notify('FFF: Failed to load main module: ' .. tostring(picker), vim.log.levels.ERROR)
          return
        end
        local change_ok, err = pcall(picker.change_indexing_directory, new_canonical)
        if not change_ok then
          vim.notify('FFF: Failed to change indexing directory: ' .. tostring(err), vim.log.levels.ERROR)
        end
      end)
    end,
    desc = 'Automatically sync FFF directory changes',
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

  local frecency_db_path = config.frecency.db_path or (vim.fn.stdpath('cache') .. '/fff_frecency')
  local history_db_path = config.history.db_path or (vim.fn.stdpath('data') .. '/fff_history')

  local ok, result = pcall(fuzzy.init_db, frecency_db_path, history_db_path, true)
  if not ok then vim.notify('Failed to databases: ' .. tostring(result), vim.log.levels.WARN) end

  ok, result = pcall(fuzzy.init_file_picker, config.base_path)
  if not ok then
    vim.notify('Failed to initialize file picker: ' .. tostring(result), vim.log.levels.ERROR)
    return fuzzy
  end

  state.file_picker_initialized = true
  setup_global_autocmds(config)

  local git_utils = require('fff.git_utils')
  git_utils.setup_highlights()

  return fuzzy
end

return M
