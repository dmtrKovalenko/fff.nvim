local plugin_dir = vim.fn.fnamemodify(vim.fn.resolve(vim.fn.expand('<sfile>:p')), ':h:h')

vim.opt.runtimepath:prepend(plugin_dir)
package.path = string.format('%s/?.lua;%s/?/init.lua;%s', plugin_dir, plugin_dir, package.path)

vim.o.swapfile = false
vim.o.backup = false
vim.o.writebackup = false

vim.cmd('cd ' .. vim.fn.fnameescape(plugin_dir))
