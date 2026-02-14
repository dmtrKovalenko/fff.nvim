--- Test normal file picker cursor highlight
--- Usage: nvim test_normal_picker.lua
--- Opens the normal file picker to verify cursor highlight works

vim.opt.runtimepath:prepend(vim.fn.getcwd())

vim.g.fff = {
  base_path = vim.fn.expand('~/dev/lightsource'),
  layout = {
    height = 0.85,
    width = 0.85,
    prompt_position = 'bottom',
    preview_position = 'right',
    preview_size = 0.5,
  },
}

vim.defer_fn(function()
  local ok, fff = pcall(require, 'fff')
  if not ok then
    vim.notify('Failed to load fff: ' .. tostring(fff), vim.log.levels.ERROR)
    return
  end

  -- Check what Visual highlight looks like
  local hl = vim.api.nvim_get_hl(0, { name = 'Visual' })
  vim.notify('Visual hl: ' .. vim.inspect(hl), vim.log.levels.INFO)

  fff.find_files()
end, 200)
