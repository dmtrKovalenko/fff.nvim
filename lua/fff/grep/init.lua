--- Grep search bridge — wraps the Rust `live_grep` FFI function
--- with file-based pagination state tracking.
---@class fff.grep
local M = {}

local fuzzy = require('fff.fuzzy')

---@class fff.grep.SearchResult
---@field items table[] Array of grep match items
---@field total_matched number Total matches found in this call
---@field total_files_searched number Files actually searched in this call
---@field total_files number Total indexed files
---@field filtered_file_count number Total searchable files after filtering
---@field next_file_offset number File offset to pass for the next page (0 = no more results)

local last_result = nil

--- Perform a grep search.
---@param query string The search query (may contain file constraints like *.rs)
---@param file_offset? number Index into sorted file list to start from (default 0)
---@param page_size? number Max matches to collect (default 50)
---@param config? table Grep configuration overrides
---@param grep_mode? string Search mode: "plain" (default), "regex", or "fuzzy"
---@return fff.grep.SearchResult
function M.search(query, file_offset, page_size, config, grep_mode)
  local conf = config or {}
  last_result = fuzzy.live_grep(
    query or '',
    file_offset or 0,
    page_size or 50,
    conf.max_file_size,
    conf.max_matches_per_file,
    conf.smart_case,
    grep_mode or 'plain',
    conf.time_budget_ms
  )
  return last_result
end

--- Get metadata from the last search result.
---@return { total_matched: number, total_files_searched: number, total_files: number, next_file_offset: number }
function M.get_search_metadata()
  if not last_result then
    return { total_matched = 0, total_files_searched = 0, total_files = 0, next_file_offset = 0 }
  end
  return {
    total_matched = last_result.total_matched or 0,
    total_files_searched = last_result.total_files_searched or 0,
    total_files = last_result.total_files or 0,
    next_file_offset = last_result.next_file_offset or 0,
  }
end

return M
