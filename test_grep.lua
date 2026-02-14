--- E2E test for live grep functionality
--- Usage: nvim -l test_grep.lua
--- Opens the live grep picker at ~/dev/lightsource

-- Add the plugin to runtimepath so require('fff') works
vim.opt.runtimepath:prepend(vim.fn.getcwd())

-- Minimal setup
vim.g.fff = {
  base_path = vim.fn.expand('~/dev/lightsource'),
  layout = {
    height = 0.85,
    width = 0.85,
    prompt_position = 'bottom',
    preview_position = 'right',
    preview_size = 0.5,
  },
  grep = {
    max_file_size = 10 * 1024 * 1024,
    smart_case = true,
    max_matches_per_file = 200,
  },
}

-- Defer opening so Neovim is fully initialized
vim.defer_fn(function()
  local ok, fff = pcall(require, 'fff')
  if not ok then
    vim.notify('Failed to load fff: ' .. tostring(fff), vim.log.levels.ERROR)
    return
  end

  fff.live_grep()
end, 200)
