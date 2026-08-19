local plugin_dir = vim.fn.fnamemodify(vim.fn.resolve(vim.fn.expand('<sfile>:p')), ':h:h')
local upstream_dir = vim.env.FFF_UPSTREAM

assert(upstream_dir and upstream_dir ~= '', 'FFF_UPSTREAM must point to an upstream fff checkout')
assert(vim.fn.isdirectory(upstream_dir) == 1, 'FFF_UPSTREAM is not a directory: ' .. upstream_dir)

vim.opt.runtimepath:prepend(upstream_dir)
vim.opt.runtimepath:prepend(plugin_dir)
package.path = string.format(
  '%s/?.lua;%s/?/init.lua;%s/?.lua;%s/?/init.lua;%s',
  plugin_dir,
  plugin_dir,
  upstream_dir,
  upstream_dir,
  package.path
)

vim.o.swapfile = false
vim.o.backup = false
vim.o.writebackup = false

vim.cmd('cd ' .. vim.fn.fnameescape(plugin_dir))

-- The integration test exercises upstream's real Lua modules. The compiled
-- Rust library is replaced at its documented seam so the smoke test stays
-- portable and does not download or build release artifacts.
package.preload['fff.rust'] = function()
  return {
    init_db = function() end,
    init_file_picker = function() end,
    init_tracing = function() return nil end,
    get_file_access_count = function() return 0 end,
    hex_dump = function() return {} end,
  }
end
