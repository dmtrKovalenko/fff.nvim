local M = {}

local function is_boundary(char) return char == '' or char:find('[%s%p]') ~= nil end

local function prepare(value, case_sensitive)
  value = tostring(value or '')
  return case_sensitive and value or value:lower()
end

local function fuzzy(text, query, case_sensitive)
  text = prepare(text, case_sensitive)
  query = prepare(query, case_sensitive)
  if query == '' then return { score = 0, positions = {} } end

  local score = 0
  local position = 1
  local previous = 0
  local positions = {}
  for index = 1, #query do
    local found = text:find(query:sub(index, index), position, true)
    if not found then return nil end
    table.insert(positions, found)
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
  return { score = score - (#text * 0.01), positions = positions }
end

local function contiguous(text, query, mode, case_sensitive)
  text = prepare(text, case_sensitive)
  query = prepare(query, case_sensitive)
  if query == '' then return { score = 0, positions = {} } end

  local start
  if mode == 'prefix' then
    start = text:sub(1, #query) == query and 1 or nil
  elseif mode == 'suffix' then
    start = text:sub(-#query) == query and #text - #query + 1 or nil
  else
    start = text:find(query, 1, true)
  end
  if not start then return nil end

  local positions = {}
  for index = start, start + #query - 1 do
    table.insert(positions, index)
  end
  local bonus = mode == 'prefix' and 180 or (mode == 'suffix' and 150 or 120)
  return { score = bonus - start - (#text * 0.01), positions = positions }
end

local function parse_term(token)
  local excluded = token:sub(1, 1) == '!'
  if excluded then token = token:sub(2) end

  local field, value = token:match('^([%a_][%w_]*):(.*)$')
  if field then token = value end

  local mode = 'fuzzy'
  if token:sub(1, 1) == "'" then
    mode = 'exact'
    token = token:sub(2)
  elseif token:sub(1, 1) == '^' then
    mode = 'prefix'
    token = token:sub(2)
  elseif token:sub(-1) == '$' then
    mode = 'suffix'
    token = token:sub(1, -2)
  elseif field then
    mode = 'exact'
  end

  return {
    value = token,
    field = field,
    excluded = excluded,
    mode = mode,
    case_sensitive = token:find('%u') ~= nil,
  }
end

function M.parse(query)
  local terms = {}
  for token in tostring(query or ''):gmatch('%S+') do
    local term = parse_term(token)
    if term.value ~= '' then table.insert(terms, term) end
  end
  return terms
end

local function match_term(value, term)
  if term.mode == 'fuzzy' then return fuzzy(value, term.value, term.case_sensitive) end
  return contiguous(value, term.value, term.mode, term.case_sensitive)
end

local function merge_positions(target, positions)
  local seen = {}
  for _, position in ipairs(target) do
    seen[position] = true
  end
  for _, position in ipairs(positions) do
    if not seen[position] then
      seen[position] = true
      table.insert(target, position)
    end
  end
  table.sort(target)
end

function M.match(text, query, fields)
  local terms = M.parse(query)
  if #terms == 0 then return { score = 0, positions = {} } end

  local result = { score = 0, positions = {} }
  for _, term in ipairs(terms) do
    local value = term.field and fields and fields[term.field] or text
    local matched = value ~= nil and match_term(value, term) or nil
    if term.excluded then
      if matched then return nil end
    elseif not matched then
      return nil
    else
      result.score = result.score + matched.score
      if not term.field then merge_positions(result.positions, matched.positions) end
    end
  end
  return result
end

function M.score(text, query, fields)
  local result = M.match(text, query, fields)
  return result and result.score or nil
end

function M.filter(items, query, get_text, get_fields)
  if not query or query == '' then return items, {} end

  local matches = {}
  for index, item in ipairs(items) do
    local fields = get_fields and get_fields(item) or (type(item) == 'table' and item or nil)
    local result = M.match(get_text(item), query, fields)
    if result then table.insert(matches, { item = item, result = result, index = index }) end
  end

  table.sort(matches, function(left, right)
    if left.result.score == right.result.score then return left.index < right.index end
    return left.result.score > right.result.score
  end)

  local filtered = {}
  local details = {}
  for _, match in ipairs(matches) do
    table.insert(filtered, match.item)
    details[match.item] = match.result
  end
  return filtered, details
end

return M
