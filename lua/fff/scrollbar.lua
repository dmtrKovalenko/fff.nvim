--- Scrollbar module for pagination indicator
local M = {}

-- Internal state
local scrollbar_state = {
  win = nil,
  buf = nil,
  ever_shown = false,
}

local ns_id = vim.api.nvim_create_namespace('fff_scrollbar')

--- Render the scrollbar to show current page position
--- Creates the window lazily if needed
--- @param layout table Layout info with list_col, list_row, list_width, list_height, show_scrollbar
--- @param config table Config with hl (highlight groups)
--- @param list_win number List window handle
--- @param pagination table Pagination state with page_index, page_size, total_matched
--- @param prompt_position string|nil Prompt position ('top' or 'bottom', defaults to 'bottom')
function M.render(layout, config, list_win, pagination, prompt_position)
  if layout.show_scrollbar == false then return end

  -- this is the most often path, we don't want to show scrollbar if use doesn't scrolling
  if not scrollbar_state.ever_shown and pagination.page_index == 0 then return end

  prompt_position = prompt_position or 'bottom'

  local total_pages = pagination.page_size > 0 and math.ceil(pagination.total_matched / pagination.page_size) or 1
  local has_multiple_pages = total_pages > 1
  local scrollbar_exists = scrollbar_state.win and vim.api.nvim_win_is_valid(scrollbar_state.win)

  -- If only one page, hide existing scrollbar and return
  if not has_multiple_pages then
    if scrollbar_exists then pcall(vim.api.nvim_win_hide, scrollbar_state.win) end
    return
  end

  -- rendering in a separate buffer to overflow the border
  if not scrollbar_exists then
    scrollbar_state.buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_option(scrollbar_state.buf, 'bufhidden', 'wipe')

    scrollbar_state.win = vim.api.nvim_open_win(scrollbar_state.buf, false, {
      relative = 'editor',
      width = 1,
      height = layout.list_height,
      col = layout.list_col + layout.list_width + 1,
      row = layout.list_row + 1,
      border = 'none',
      style = 'minimal',
      focusable = false,
    })

    local scrollbar_hl = string.format('Normal:%s', config.hl.border)
    vim.api.nvim_win_set_option(scrollbar_state.win, 'winhighlight', scrollbar_hl)

    scrollbar_state.ever_shown = true
  end

  if not scrollbar_state.buf or not vim.api.nvim_buf_is_valid(scrollbar_state.buf) then return end
  pcall(vim.api.nvim_win_set_config, scrollbar_state.win, { hide = false })

  local win_height = vim.api.nvim_win_get_height(list_win)

  local thumb_size = math.max(1, math.floor(win_height / total_pages))
  local scrollbar_range = win_height - thumb_size

  -- inverse the scrollbar when the position is at the bottom
  local thumb_start
  if prompt_position == 'bottom' then
    thumb_start =
      math.floor(((total_pages - 1 - pagination.page_index) / math.max(1, total_pages - 1)) * scrollbar_range)
  else
    thumb_start = math.floor((pagination.page_index / math.max(1, total_pages - 1)) * scrollbar_range)
  end

  local lines = {}
  for i = 1, win_height do
    if i >= thumb_start + 1 and i < thumb_start + thumb_size + 1 then
      table.insert(lines, '▊') -- Thick block for thumb
    else
      table.insert(lines, '│') -- Thin line for track
    end
  end

  pcall(vim.api.nvim_buf_set_option, scrollbar_state.buf, 'modifiable', true)
  pcall(vim.api.nvim_buf_set_lines, scrollbar_state.buf, 0, -1, false, lines)
  pcall(vim.api.nvim_buf_set_option, scrollbar_state.buf, 'modifiable', false)

  pcall(vim.api.nvim_buf_clear_namespace, scrollbar_state.buf, ns_id, 0, -1)
  if thumb_size > 0 then
    pcall(vim.api.nvim_buf_set_extmark, scrollbar_state.buf, ns_id, thumb_start, 0, {
      end_row = thumb_start + thumb_size,
      end_col = 0,
      hl_group = config.hl.scrollbar,
      hl_eol = true,
    })
  end
end

function M.cleanup()
  if scrollbar_state.win and vim.api.nvim_win_is_valid(scrollbar_state.win) then
    pcall(vim.api.nvim_win_close, scrollbar_state.win, true)
  end

  if scrollbar_state.buf and vim.api.nvim_buf_is_valid(scrollbar_state.buf) then
    pcall(vim.api.nvim_buf_delete, scrollbar_state.buf, { force = true })
  end

  scrollbar_state.win = nil
  scrollbar_state.buf = nil
  scrollbar_state.ever_shown = false
end

return M
