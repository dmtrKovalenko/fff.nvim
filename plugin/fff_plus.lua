if vim.g.fff_plus_loaded then return end
vim.g.fff_plus_loaded = true

local config = vim.g.fff_plus

if type(config) == 'table' then
  require('fff_plus').setup(config)
else
  require('fff_plus').register_commands()
end
