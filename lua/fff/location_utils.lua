local M = {}

--- Jump to a location in the current buffer
--- @param location table|nil Location data from search results
function M.jump_to_location(location)
  if not location then return end

  local current_buf = vim.api.nvim_get_current_buf()
  local line_count = vim.api.nvim_buf_line_count(current_buf)

  if location.line then
    local target_line = math.max(1, math.min(location.line, line_count))
    local target_col = location.col and math.max(0, location.col - 1) or 0

    vim.api.nvim_win_set_cursor(0, { target_line, target_col })
    vim.cmd('normal! zz')
  elseif location.start and location['end'] then
    -- Extract line numbers from nested structure
    local start_line = math.max(1, math.min(location.start.line, line_count))
    local end_line = math.max(start_line, math.min(location['end'].line, line_count))

    -- start in the visual mode and selecting the range backwards so the cursor ends up at the start
    vim.api.nvim_win_set_cursor(0, { end_line, 0 })
    vim.cmd('normal! V')
    if end_line > start_line then vim.cmd('normal! ' .. (end_line - start_line) .. 'k') end
    vim.cmd('normal! zz')
  end
end

--- Highlight a location range in a buffer using extmarks
--- @param bufnr number Buffer number
--- @param location table|nil Location data from search results
--- @param namespace number Namespace for extmarks
--- @return table|nil Highlight extmark details for cleanup
function M.highlight_location(bufnr, location, namespace)
  if not location or not vim.api.nvim_buf_is_valid(bufnr) then return nil end

  local line_count = vim.api.nvim_buf_line_count(bufnr)
  local extmarks = {}

  if location.line then
    local target_line = math.max(1, math.min(location.line, line_count))

    if location.col then
      local target_col = math.max(0, location.col - 1)
      local line_content = vim.api.nvim_buf_get_lines(bufnr, target_line - 1, target_line, false)[1] or ''
      local end_col = math.min(target_col + 1, #line_content)

      local ok, mark_id = pcall(vim.api.nvim_buf_set_extmark, bufnr, namespace, target_line - 1, target_col, {
        end_col = end_col,
        hl_group = 'IncSearch', -- inc search are better visible for a single chars
        priority = 1000,
      })

      if ok then table.insert(extmarks, { id = mark_id, line = target_line - 1 }) end
    else
      local ok, mark_id = pcall(vim.api.nvim_buf_set_extmark, bufnr, namespace, target_line - 1, 0, {
        line_hl_group = 'Visual',
        priority = 1000,
      })

      if ok then table.insert(extmarks, { id = mark_id, line = target_line - 1 }) end
    end
  elseif location.start and location['end'] then
    local start_line = math.max(1, math.min(location.start.line, line_count))
    local end_line = math.max(start_line, math.min(location['end'].line, line_count))

    -- Check if we have column information for exact range highlighting
    if location.start.col and location['end'].col then
      if start_line == end_line then
        -- Single line range with columns: highlight exact character range
        local start_col = math.max(0, location.start.col - 1)
        local end_col = location['end'].col - 1
        local line_content = vim.api.nvim_buf_get_lines(bufnr, start_line - 1, start_line, false)[1] or ''
        end_col = math.min(end_col, #line_content)

        local ok, mark_id = pcall(vim.api.nvim_buf_set_extmark, bufnr, namespace, start_line - 1, start_col, {
          end_col = end_col,
          hl_group = 'IncSearch',
          priority = 1000,
        })

        if ok then table.insert(extmarks, { id = mark_id, line = start_line - 1 }) end
      else
        -- Multi-line range with exact columns: highlight precise ranges
        for line = start_line, end_line do
          local line_start_col, line_end_col

          if line == start_line then
            -- First line: from start_col to end of line
            line_start_col = math.max(0, location.start.col - 1)
            local line_content = vim.api.nvim_buf_get_lines(bufnr, line - 1, line, false)[1] or ''
            line_end_col = #line_content
          elseif line == end_line then
            -- Last line: from beginning to end_col
            line_start_col = 0
            line_end_col = location['end'].col - 1
            local line_content = vim.api.nvim_buf_get_lines(bufnr, line - 1, line, false)[1] or ''
            line_end_col = math.min(line_end_col, #line_content)
          else
            -- Middle lines: entire line
            line_start_col = 0
            local line_content = vim.api.nvim_buf_get_lines(bufnr, line - 1, line, false)[1] or ''
            line_end_col = #line_content
          end

          local ok, mark_id = pcall(vim.api.nvim_buf_set_extmark, bufnr, namespace, line - 1, line_start_col, {
            end_col = line_end_col,
            hl_group = 'Visual',
            priority = 1000,
          })

          if ok then table.insert(extmarks, { id = mark_id, line = line - 1 }) end
        end
      end
    else
      -- Multi-line or no columns: highlight entire lines
      for line = start_line, end_line do
        local ok, mark_id = pcall(vim.api.nvim_buf_set_extmark, bufnr, namespace, line - 1, 0, {
          line_hl_group = 'Visual',
          priority = 1000,
        })

        if ok then table.insert(extmarks, { id = mark_id, line = line - 1 }) end
      end
    end
  end

  return #extmarks > 0 and extmarks or nil
end

--- Clear location highlights from a buffer
--- @param bufnr number Buffer number
--- @param namespace number Namespace for extmarks
function M.clear_location_highlights(bufnr, namespace)
  if vim.api.nvim_buf_is_valid(bufnr) then vim.api.nvim_buf_clear_namespace(bufnr, namespace, 0, -1) end
end

--- Get the target line for scrolling preview to location
--- @param location table|nil Location data from search results
--- @return number|nil Target line number (1-indexed) for scrolling
function M.get_target_line(location)
  if not location then return nil end

  if location.line then
    return location.line
  elseif location.start then
    return location.start.line
  end

  return nil
end

--- Check if location is valid for a buffer
--- @param location table|nil Location data
--- @param bufnr number Buffer number
--- @return boolean True if location is valid for the buffer
function M.is_valid_location(location, bufnr)
  if not location or not vim.api.nvim_buf_is_valid(bufnr) then return false end

  local line_count = vim.api.nvim_buf_line_count(bufnr)

  if location.line then
    return location.line > 0 and location.line <= line_count
  elseif location.start and location['end'] then
    return location.start.line > 0 and location.start.line <= line_count
  end

  return false
end

--- Format location for display
--- @param location table|nil Location data
--- @return string Formatted location string
function M.format_location(location)
  if not location then return '' end

  if location.line and location.col then
    return string.format(':%d:%d', location.line, location.col)
  elseif location.line then
    return string.format(':%d', location.line)
  elseif location.start and location['end'] then
    -- Handle nested structure with optional column information
    if location.start.col and location['end'].col then
      return string.format(
        ':%d:%d-%d:%d',
        location.start.line,
        location.start.col,
        location['end'].line,
        location['end'].col
      )
    else
      return string.format(':%d-%d', location.start.line, location['end'].line)
    end
  end

  return ''
end

return M
