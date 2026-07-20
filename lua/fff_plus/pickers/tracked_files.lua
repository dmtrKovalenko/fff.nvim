local M = {}

function M.open(opts)
  opts = vim.tbl_deep_extend('force', { source = 'tracked', title = 'Git Files' }, opts or {})
  require('fff_plus.pickers.git_files').open(opts)
end

return M
