--- Minimal test runner for plenary busted-style tests
--- Usage: nvim --headless -u tests/minimal_init.lua -c "PlenaryBustedDirectory tests/ {minimal_init = 'tests/minimal_init.lua'}"

-- Set up runtimepath to include the plugin and plenary
local plugin_dir = vim.fn.fnamemodify(vim.fn.resolve(vim.fn.expand('<sfile>:p')), ':h:h')
local plenary_dir = os.getenv('PLENARY_DIR') or (plugin_dir .. '/../plenary.nvim')

vim.opt.runtimepath:prepend(plugin_dir)
vim.opt.runtimepath:prepend(plenary_dir)

-- Disable swap files and other noise for testing
vim.o.swapfile = false
vim.o.backup = false
vim.o.writebackup = false

-- Set cwd to the plugin directory so test_dir resolution is reliable
vim.cmd('cd ' .. vim.fn.fnameescape(plugin_dir))
