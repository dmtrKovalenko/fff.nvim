local M = {}

function M.toggle(selected, key)
  if selected[key] then
    selected[key] = nil
    return false
  end

  selected[key] = true
  return true
end

function M.collect(items, selected, current, get_key)
  local chosen = {}
  for _, item in ipairs(items) do
    if selected[get_key(item)] then table.insert(chosen, item) end
  end

  if #chosen == 0 and current then table.insert(chosen, current) end
  return chosen
end

function M.put(items, get_text)
  local lines = {}
  for _, item in ipairs(items) do
    table.insert(lines, get_text(item))
  end
  if #lines > 0 then vim.api.nvim_put(lines, 'l', true, true) end
end

return M
