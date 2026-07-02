local M = {}

local registry = {
  buffers = 'fff.pickers.buffers',
  colors = 'fff.pickers.colors',
  git_files = 'fff.pickers.git_files',
}

--- Open a fork-owned picker by name.
--- @param name string
--- @param opts? table
--- @return boolean
function M.open(name, opts)
  vim.validate({
    name = { name, 'string' },
    opts = { opts, 'table', true },
  })

  local module_name = registry[name]
  if not module_name then
    vim.notify('FFF: unknown picker "' .. name .. '"', vim.log.levels.ERROR)
    return false
  end

  local ok, picker = pcall(require, module_name)
  if not ok then
    vim.notify('FFF: failed to load ' .. name .. ' picker: ' .. tostring(picker), vim.log.levels.ERROR)
    return false
  end

  if type(picker.open) ~= 'function' then
    vim.notify('FFF: picker "' .. name .. '" does not expose open(opts)', vim.log.levels.ERROR)
    return false
  end

  picker.open(opts)
  return true
end

return M
