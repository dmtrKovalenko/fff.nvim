--- Oldfiles module for FFF
--- Provides recent/old files list using vim.v.oldfiles as source
local M = {}

--- Build a FileItem-compatible table from a file path
--- @param filepath string Absolute file path
--- @param base_path string|nil Base path for computing relative_path
--- @return table FileItem-compatible table
local function build_file_item(filepath, base_path)
  local name = vim.fn.fnamemodify(filepath, ':t')
  local extension = vim.fn.fnamemodify(filepath, ':e')

  local relative_path = filepath
  if base_path and vim.startswith(filepath, base_path .. '/') then
    relative_path = filepath:sub(#base_path + 2)
  end

  local stat = vim.uv.fs_stat(filepath)

  return {
    relative_path = relative_path,
    name = name,
    extension = extension,
    size = stat and stat.size or 0,
    modified = stat and stat.mtime and stat.mtime.sec or 0,
    total_frecency_score = 0,
    access_frecency_score = 0,
    modification_frecency_score = 0,
    git_status = nil,
    is_binary = false,
  }
end

--- Simple fuzzy match: all query chars must appear in order within the target
--- @param query string Lowercase query
--- @param target string Target string to match against
--- @return boolean
local function fuzzy_match(query, target)
  if query == '' then return true end
  local lower_target = target:lower()
  local ti = 1
  for qi = 1, #query do
    local ch = query:sub(qi, qi)
    local found = lower_target:find(ch, ti, true)
    if not found then return false end
    ti = found + 1
  end
  return true
end

--- Get oldfiles filtered by query
--- @param query string Search query for filtering
--- @param max_results number|nil Maximum results to return
--- @return table[] List of FileItem-compatible tables
function M.search(query, max_results)
  max_results = max_results or 100
  local lower_query = (query or ''):lower()
  local results = {}

  for _, filepath in ipairs(vim.v.oldfiles) do
    if #results >= max_results then break end

    local expanded = vim.fn.expand(filepath)
    if vim.fn.filereadable(expanded) == 1 then
      local display = vim.fn.fnamemodify(expanded, ':~')
      if fuzzy_match(lower_query, display) then
        table.insert(results, build_file_item(expanded, nil))
      end
    end
  end

  return results
end

return M
