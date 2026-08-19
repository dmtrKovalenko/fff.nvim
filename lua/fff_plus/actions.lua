local M = { registry = {} }

function M.register(name, action)
  vim.validate({ name = { name, 'string' }, action = { action, 'function' } })
  M.registry[name] = action
end

function M.get(name, overrides) return (overrides and overrides[name]) or M.registry[name] end

function M.run(instance, name, overrides, ...)
  local action = M.get(name, overrides)
  if not action then return false end
  action(instance, instance.current and instance:current() or nil, ...)
  return true
end

M.register('refresh', function(instance) instance:refresh() end)
M.register('select_all', function(instance) instance:select_all() end)
M.register('close', function(instance) instance:close(false) end)

return M
