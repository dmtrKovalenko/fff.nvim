local M = {}

local registry = {
  buffers = 'fff_plus.pickers.buffers',
  colors = 'fff_plus.pickers.colors',
  git_files = 'fff_plus.pickers.git_files',
  tracked_files = 'fff_plus.pickers.tracked_files',
}

local default_config = {
  legacy_commands = false,
}

M.config = vim.deepcopy(default_config)

local function require_upstream()
  local ok, upstream = pcall(require, 'fff')
  if ok then return upstream end

  vim.notify(
    'fff-plus.nvim requires upstream fff.nvim to be installed and loaded first: ' .. tostring(upstream),
    vim.log.levels.ERROR
  )
  return nil
end

local function create_command(name, callback, opts)
  if vim.fn.exists(':' .. name) == 2 then return end
  vim.api.nvim_create_user_command(name, callback, opts or {})
end

function M.setup(opts)
  M.config = vim.tbl_deep_extend('force', vim.deepcopy(default_config), opts or {})
  require_upstream()
  M.register_commands()
end

function M.open(name, opts)
  vim.validate({
    name = { name, 'string' },
    opts = { opts, 'table', true },
  })

  if not require_upstream() then return false end

  local module_name = registry[name]
  if not module_name then
    vim.notify('fff-plus.nvim: unknown picker "' .. name .. '"', vim.log.levels.ERROR)
    return false
  end

  local ok, picker = pcall(require, module_name)
  if not ok then
    vim.notify('fff-plus.nvim: failed to load ' .. name .. ' picker: ' .. tostring(picker), vim.log.levels.ERROR)
    return false
  end

  if type(picker.open) ~= 'function' then
    vim.notify('fff-plus.nvim: picker "' .. name .. '" does not expose open(opts)', vim.log.levels.ERROR)
    return false
  end

  picker.open(opts)
  return true
end

function M.buffers(opts) return M.open('buffers', opts) end

function M.colors(opts) return M.open('colors', opts) end

function M.git_files(opts) return M.open('git_files', opts) end

function M.git_status(opts) return M.open('git_files', opts) end

function M.tracked_files(opts) return M.open('tracked_files', opts) end

function M.register_commands()
  create_command(
    'FFFPlusBuffers',
    function(opts)
      M.buffers({
        title = 'Buffers',
        prompt = opts.args ~= '' and opts.args or nil,
      })
    end,
    {
      nargs = '?',
      desc = 'Browse and switch between open buffers with fff-plus.nvim',
    }
  )

  create_command('FFFPlusColors', function(opts)
    M.colors({
      bang = opts.bang,
    })
  end, {
    bang = true,
    desc = 'Browse and switch colorschemes with fff-plus.nvim',
  })

  create_command('FFFPlusGFiles', function() M.git_files() end, {
    bang = true,
    desc = 'Browse Git status files with fff-plus.nvim (compatibility name)',
  })

  create_command('FFFPlusGitFiles', function() M.tracked_files() end, {
    bang = true,
    desc = 'Browse files tracked by Git with fff-plus.nvim',
  })

  create_command('FFFPlusGitStatus', function() M.git_status() end, {
    bang = true,
    desc = 'Browse Git status files with fff-plus.nvim',
  })

  if not M.config.legacy_commands then return end

  create_command(
    'FFFBuffers',
    function(opts)
      M.buffers({
        title = 'Buffers',
        prompt = opts.args ~= '' and opts.args or nil,
      })
    end,
    {
      nargs = '?',
      desc = 'Browse and switch between open buffers with fff-plus.nvim',
    }
  )

  create_command('Colors', function(opts)
    M.colors({
      bang = opts.bang,
    })
  end, {
    bang = true,
    desc = 'Browse and switch colorschemes with fff-plus.nvim',
  })

  create_command('GFiles', function() M.tracked_files() end, {
    bang = true,
    desc = 'Browse files tracked by Git with fff-plus.nvim',
  })
end

return M
