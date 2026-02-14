--- E2E test for list_renderer extraction
--- Tests both file picker and grep modes with both prompt positions.
---
--- Usage:
---   nvim -l test_list_renderer.lua              (bottom prompt, file picker)
---   nvim -l test_list_renderer.lua top           (top prompt, file picker)
---   nvim -l test_list_renderer.lua bottom grep   (bottom prompt, grep)
---   nvim -l test_list_renderer.lua top grep      (top prompt, grep)

vim.opt.runtimepath:prepend(vim.fn.getcwd())

local args = vim.fn.argv()
local prompt_pos = 'bottom'
local mode = 'files'

for _, arg in ipairs(args) do
  if arg == 'top' then prompt_pos = 'top' end
  if arg == 'grep' then mode = 'grep' end
end

vim.g.fff = {
  base_path = vim.fn.expand('~/dev/lightsource'),
  layout = {
    height = 0.85,
    width = 0.85,
    prompt_position = prompt_pos,
    preview_position = 'right',
    preview_size = 0.5,
  },
  grep = {
    max_file_size = 10 * 1024 * 1024,
    smart_case = true,
    max_matches_per_file = 200,
  },
}

vim.defer_fn(function()
  local ok, fff = pcall(require, 'fff')
  if not ok then
    vim.notify('Failed to load fff: ' .. tostring(fff), vim.log.levels.ERROR)
    return
  end

  vim.notify(string.format('list_renderer test: prompt=%s mode=%s', prompt_pos, mode), vim.log.levels.INFO)

  if mode == 'grep' then
    fff.live_grep()
  else
    fff.find_files()
  end
end, 200)
