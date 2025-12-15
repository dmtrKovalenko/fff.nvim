-- Single file Neovim config for testing fff.nvim locally
-- Usage: nvim -u /Users/neogoose/dev/fff.nvim/init.lua

-- Set up lazy.nvim plugin manager
local lazypath = vim.fn.stdpath('data') .. '/lazy/lazy.nvim'
if not vim.loop.fs_stat(lazypath) then
  vim.fn.system({
    'git',
    'clone',
    '--filter=blob:none',
    'https://github.com/folke/lazy.nvim.git',
    '--branch=stable',
    lazypath,
  })
end
vim.opt.rtp:prepend(lazypath)

require('lazy').setup({
  {
    dir = '~/dev/fff.nvim',
    'https://github.com/dmtrKovalenko/fff.nvim',
    build = function()
      -- this will download prebuild binary or try to use existing rustup toolchain to build from source
      -- (if you are using lazy you can use gb for rebuilding a plugin if needed)
      require('fff.download').download_or_build_binary()
    end,
    dependencies = {
      'nvim-tree/nvim-web-devicons', -- Optional: for file icons
      -- {
      --   'nvim-mini/mini.icons',
      --   version = false,
      --   config = true,
      -- },
    },
    config = function()
      require('fff').setup({
        -- Configure fff.nvim here
        ui = {
          width = 0.8,
          height = 0.8,
        },
        file_picker = {
          auto_reload_on_write = true,
          frecency_boost = true,
        },
      })
    end,
  },
}, {
  root = vim.fn.stdpath('data') .. '/fff-empty-test',
  lockfile = vim.fn.stdpath('data') .. '/fff-empty-test.json',
})

vim.opt.number = true
vim.opt.relativenumber = true

vim.keymap.set('n', 'ff', function() require('fff').find_files() end, { desc = 'Find files' })
vim.keymap.set('n', 'fg', function() require('fff').find_in_git_root() end, { desc = 'Find files in git root' })
vim.keymap.set('n', 'fr', function() require('fff').scan_files() end, { desc = 'Rescan files' })
vim.keymap.set('n', 'fs', function() require('fff').refresh_git_status() end, { desc = 'Refresh git status' })

vim.notify('FFF.nvim local config loaded! Press ff', vim.log.levels.INFO)
