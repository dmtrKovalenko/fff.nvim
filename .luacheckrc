-- luacheck configuration for fff.nvim
-- https://luacheck.readthedocs.io/en/stable/config.html

-- Neovim globals
globals = { "vim" }

-- Standard library
std = "luajit"

-- Ignore line length (handled by stylua)
max_line_length = false

-- Ignore unused self argument in methods
self = false

-- Files/directories to ignore
exclude_files = {
  ".luarocks/",
}

-- Warn about unused variables, but allow _ prefix convention
unused_args = true
ignore = {
  "212", -- unused argument (too noisy for callback-heavy code)
}
