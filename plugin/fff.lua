if vim.g.fff_loaded then
  return
end
vim.g.fff_loaded = true

-- Defer indexing until after UIEnter, so as to not block the UI.
-- This is equivalent to lazy.nvim's VeryLazy, but works with all plugin managers.

if vim.v.vim_did_enter == 1 then
  require('fff.main')
else
  vim.api.nvim_create_autocmd('UIEnter', {
    group = vim.api.nvim_create_augroup('fff.main', {}),
    once = true,
    nested = true,
    callback = vim.schedule_wrap(function()
    if vim.v.exiting ~= vim.NIL then
       return
    end
    require('fff.main')
    end),
  })
end
