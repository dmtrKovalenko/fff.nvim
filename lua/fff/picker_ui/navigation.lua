local M = {}

local list_separator = require('fff.list_separator')
local preview = require('fff.file_picker.preview')
local layout = require('fff.layout')
local picker_ui_state = require('fff.picker_ui.picker_ui_state')

-- Parent module reference (set by picker_ui.lua during initialization).
---@type table
local P = nil

function M.init(parent_module) P = parent_module end

local S = picker_ui_state.state

local function get_prompt_position() return layout.resolve_prompt_position(S.config) end

--- After cursor moves, decide whether the combo separator should hide.
local function maybe_hide_combo_separator()
  if not (S.combo_initial_cursor and S.combo_visible) then return end
  local distance_past = S.cursor - S.combo_initial_cursor
  if distance_past == 0 then return end
  local half_page = math.floor(S.pagination.page_size * 0.5)
  if math.abs(distance_past) <= half_page then return end

  S.combo_visible = false
  list_separator.hide()
  P.render_list()
  if get_prompt_position() == 'bottom' then P.scroll_to_bottom() end
end

function M.wrap_to_first()
  if S.pagination.page_index == 0 then
    S.cursor = 1
    return true
  end

  if S.mode ~= 'grep' then
    return P.load_page_at_index(0, function() S.cursor = 1 end)
  end

  if S.pagination.grep_file_offsets[1] ~= nil then
    return P.load_page_at_index(0, function() S.cursor = 1 end)
  end

  S.cursor = 1
  return true
end

function M.wrap_to_last()
  local page_size = S.pagination.page_size
  if page_size == 0 then return false end

  if S.mode ~= 'grep' then
    local total = S.pagination.total_matched
    if total == 0 then return false end
    local max_page_index = math.max(0, math.ceil(total / page_size) - 1)

    if S.pagination.page_index == max_page_index then
      S.cursor = #S.filtered_items
      return true
    end

    return P.load_page_at_index(max_page_index, function(result_count) S.cursor = result_count end)
  end

  S.cursor = #S.filtered_items
  return true
end

function M.move_up()
  if not P.state.active then return end
  if #S.filtered_items == 0 then return end

  local prompt_position = get_prompt_position()
  local items_count = #S.filtered_items
  local old_cursor = S.cursor
  local wrap_around = S.config and S.config.wrap_around or false

  if prompt_position == 'bottom' then
    local near_bottom = S.cursor >= (items_count - S.pagination.prefetch_margin)
    local at_last_item = S.cursor >= items_count

    if near_bottom and at_last_item then
      local page_size = S.pagination.page_size
      local has_more = false
      if page_size > 0 then
        if S.mode == 'grep' then
          has_more = S.pagination.grep_next_file_offset > 0
        else
          local max_page = math.max(0, math.ceil(S.pagination.total_matched / page_size) - 1)
          has_more = S.pagination.page_index < max_page
        end
      end

      if has_more then
        P.load_next_page()
        return
      elseif wrap_around then
        M.wrap_to_first()
      end
    else
      S.cursor = math.min(S.cursor + 1, items_count)
    end
  else
    if S.cursor <= S.pagination.prefetch_margin + 1 and S.cursor <= 1 then
      if S.pagination.page_index > 0 then
        vim.schedule(P.load_previous_page)
        return
      elseif wrap_around then
        M.wrap_to_last()
      end
    else
      S.cursor = math.max(S.cursor - 1, 1)
    end
  end

  if not P.render_after_cursor_move(old_cursor) then return end
  P.update_status()
  pcall(vim.cmd, 'redraw')
  P.update_preview_debounced()

  maybe_hide_combo_separator()
end

function M.move_down()
  if not P.state.active then return end
  if #S.filtered_items == 0 then return end

  local prompt_position = get_prompt_position()
  local items_count = #S.filtered_items
  local old_cursor = S.cursor
  local wrap_around = S.config and S.config.wrap_around or false

  if prompt_position == 'bottom' then
    if S.cursor <= S.pagination.prefetch_margin + 1 and S.cursor <= 1 then
      if S.pagination.page_index > 0 then
        vim.schedule(P.load_previous_page)
        return
      elseif wrap_around then
        M.wrap_to_last()
      end
    else
      S.cursor = math.max(S.cursor - 1, 1)
    end
  else
    local near_bottom = S.cursor >= (items_count - S.pagination.prefetch_margin)
    local at_last_item = S.cursor >= items_count

    if near_bottom and at_last_item then
      local page_size = S.pagination.page_size
      local has_more = false
      if page_size > 0 then
        if S.mode == 'grep' then
          has_more = S.pagination.grep_next_file_offset > 0
        else
          local max_page = math.max(0, math.ceil(S.pagination.total_matched / page_size) - 1)
          has_more = S.pagination.page_index < max_page
        end
      end

      if has_more then
        P.load_next_page()
        return
      elseif wrap_around then
        M.wrap_to_first()
      end
    else
      S.cursor = math.min(S.cursor + 1, items_count)
    end
  end

  if not P.render_after_cursor_move(old_cursor) then return end
  P.update_status()
  pcall(vim.cmd, 'redraw')
  P.update_preview_debounced()

  maybe_hide_combo_separator()
end

function M.scroll_preview_up()
  if not P.state.active or not S.preview_win then return end

  local win_height = vim.api.nvim_win_get_height(S.preview_win)
  local scroll_lines = math.floor(win_height / 2)

  preview.scroll(-scroll_lines)
end

function M.scroll_preview_down()
  if not P.state.active or not S.preview_win then return end

  local win_height = vim.api.nvim_win_get_height(S.preview_win)
  local scroll_lines = math.floor(win_height / 2)

  preview.scroll(scroll_lines)
end

return M
