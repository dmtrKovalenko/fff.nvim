--- Grep search bridge — wraps the Rust `live_grep` FFI function
--- with pagination state tracking.
---@class fff.grep
local M = {}

local fuzzy = require('fff.fuzzy')

---@class fff.grep.SearchResult
---@field items table[] Array of grep match items
---@field total_matched number Total matches found
---@field total_files_searched number Files actually searched
---@field total_files number Total indexed files

local last_result = nil

--- Perform a grep search.
---@param query string The search query (may contain file constraints like *.rs)
---@param page_index? number 0-based page offset (default 0)
---@param page_size? number Results per page (default 50)
---@param config? table Grep configuration overrides
---@return fff.grep.SearchResult
function M.search(query, page_index, page_size, config)
  local conf = config or {}
  last_result = fuzzy.live_grep(
    query or '',
    page_index or 0,
    page_size or 50,
    conf.max_file_size,
    conf.max_matches_per_file,
    conf.smart_case
  )
  return last_result
end

--- Get metadata from the last search result.
---@return { total_matched: number, total_files_searched: number, total_files: number }
function M.get_search_metadata()
  if not last_result then return { total_matched = 0, total_files_searched = 0, total_files = 0 } end
  return {
    total_matched = last_result.total_matched or 0,
    total_files_searched = last_result.total_files_searched or 0,
    total_files = last_result.total_files or 0,
  }
end

return M
