local M = {}

local function is_boundary(char) return char == '' or char:find('[%s%p]') ~= nil end

function M.score(text, query)
  text = tostring(text or ''):lower()
  query = tostring(query or ''):lower()
  if query == '' then return 0 end

  local score = 0
  local position = 1
  local previous = 0

  for index = 1, #query do
    local found = text:find(query:sub(index, index), position, true)
    if not found then return nil end

    score = score + 10
    if found == previous + 1 then score = score + 8 end
    if found == 1 or is_boundary(text:sub(found - 1, found - 1)) then score = score + 6 end

    score = score - (found * 0.1)
    previous = found
    position = found + 1
  end

  local substring_start = text:find(query, 1, true)
  if substring_start then score = score + 50 - substring_start end
  if substring_start == 1 then score = score + 100 end
  if text == query then score = score + 200 end

  return score - (#text * 0.01)
end

function M.filter(items, query, get_text)
  if not query or query == '' then return items end

  local matches = {}
  for index, item in ipairs(items) do
    local score = M.score(get_text(item), query)
    if score then table.insert(matches, { item = item, score = score, index = index }) end
  end

  table.sort(matches, function(left, right)
    if left.score == right.score then return left.index < right.index end
    return left.score > right.score
  end)

  local filtered = {}
  for _, match in ipairs(matches) do
    table.insert(filtered, match.item)
  end
  return filtered
end

return M
