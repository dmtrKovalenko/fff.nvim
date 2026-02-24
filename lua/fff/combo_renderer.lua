local M = {}

local overlay_state = {
  left_buf = nil,
  left_win = nil,
  right_buf = nil,
  right_win = nil,
  ns_id = nil,
  -- Cache last position to avoid unnecessary updates
  last_row = nil,
  last_col = nil,
  last_border_hl = nil,
  -- Track if combo was rendered in last call
  was_rendered = false,
}

local LEFT_OVERLAY_CONTENT = '├────'
local RIGHT_OVERLAY_CONTENT = '─┤'
local LEFT_OVERLAY_WIDTH = vim.fn.strdisplaywidth(LEFT_OVERLAY_CONTENT)
local LEFT_HEADER_PADDING = LEFT_OVERLAY_WIDTH - 2
local RIGHT_OVERLAY_WIDTH = vim.fn.strdisplaywidth(RIGHT_OVERLAY_CONTENT)

local COMBO_TEXT_FORMAT = 'Last Match (×%d combo) '
local LAST_MATCH_TEXT_FORMAT = 'Last Match '

function M.init(ns_id) overlay_state.ns_id = ns_id end

local function detect_combo_item(items, file_picker, combo_boost_score_multiplier)
  if not items or #items == 0 then return nil, 0 end

  local first_score = file_picker.get_file_score(1)
  local last_score = file_picker.get_file_score(#items)

  if first_score.combo_match_boost > combo_boost_score_multiplier then
    return 1, first_score.combo_match_boost / combo_boost_score_multiplier
  elseif last_score.combo_match_boost > combo_boost_score_multiplier then
    return #items, last_score.combo_match_boost / combo_boost_score_multiplier
  end

  return nil, 0
end

local function create_header_text(combo_count, win_width, disable_combo_display)
  local combo_text = nil
  if disable_combo_display then
    combo_text = LAST_MATCH_TEXT_FORMAT
  else
    combo_text = string.format(COMBO_TEXT_FORMAT, combo_count)
  end

  local text_len = vim.fn.strdisplaywidth(combo_text)
  local available_for_content = win_width - LEFT_HEADER_PADDING - RIGHT_OVERLAY_WIDTH
  local remaining_dashes = math.max(0, available_for_content - text_len)

  return string.rep(' ', LEFT_HEADER_PADDING) .. combo_text .. string.rep('─', remaining_dashes), text_len
end

local function apply_header_highlights(buf, ns_id, line_idx, text_len, border_hl)
  local config = require('fff.conf').get()
  vim.api.nvim_buf_add_highlight(buf, ns_id, border_hl, line_idx - 1, 0, -1)
  vim.api.nvim_buf_add_highlight(
    buf,
    ns_id,
    config.hl.combo_header,
    line_idx - 1,
    LEFT_HEADER_PADDING,
    LEFT_HEADER_PADDING + text_len
  )
end

local function get_or_create_overlay_buf(state_key)
  if not overlay_state[state_key] or not vim.api.nvim_buf_is_valid(overlay_state[state_key]) then
    ---@diagnostic disable-next-line: assign-type-mismatch
    overlay_state[state_key] = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_option(overlay_state[state_key], 'bufhidden', 'wipe')
  end
  return overlay_state[state_key]
end

local function update_overlay_content(buf, content, border_hl)
  -- Batch all buffer operations together for performance
  vim.api.nvim_buf_set_option(buf, 'modifiable', true)
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, { content })
  vim.api.nvim_buf_clear_namespace(buf, overlay_state.ns_id, 0, -1)
  vim.api.nvim_buf_add_highlight(buf, overlay_state.ns_id, border_hl, 0, 0, -1)
  vim.api.nvim_buf_set_option(buf, 'modifiable', false)
end

local function position_overlay_window(state_key, buf, width, row, col)
  local win_config = {
    relative = 'editor',
    width = width,
    height = 1,
    row = row,
    col = col,
    style = 'minimal',
    border = 'none',
    focusable = false,
    zindex = 250,
  }

  if overlay_state[state_key] and vim.api.nvim_win_is_valid(overlay_state[state_key]) then
    vim.api.nvim_win_set_config(overlay_state[state_key], win_config)
  else
    ---@diagnostic disable-next-line: assign-type-mismatch
    overlay_state[state_key] = vim.api.nvim_open_win(buf, false, win_config)
  end

  vim.api.nvim_win_set_option(overlay_state[state_key], 'winhl', 'Normal:Normal')
end

local function update_overlays(list_win, combo_header_line, border_hl)
  local list_config = vim.api.nvim_win_get_config(list_win)
  -- combo_header_line is a 1-based buffer line index (includes any padding offset)
  -- list_config.row is where the window starts (0-based, at the top border)
  -- Content starts at list_config.row + 1 (after the top border)
  -- Buffer line 1 -> screen row (list_config.row + 1)
  -- Buffer line N -> screen row (list_config.row + N)
  -- Since combo_header_line is 1-based, the formula naturally works out
  local combo_header_row = list_config.row + combo_header_line

  -- Skip update if position and highlight haven't changed
  if
    overlay_state.last_row == combo_header_row
    and overlay_state.last_col == list_config.col
    and overlay_state.last_border_hl == border_hl
    and overlay_state.left_win
    and vim.api.nvim_win_is_valid(overlay_state.left_win)
    and overlay_state.right_win
    and vim.api.nvim_win_is_valid(overlay_state.right_win)
  then
    return
  end

  overlay_state.last_row = combo_header_row
  overlay_state.last_col = list_config.col
  overlay_state.last_border_hl = border_hl

  local left_buf = get_or_create_overlay_buf('left_buf')
  local right_buf = get_or_create_overlay_buf('right_buf')

  update_overlay_content(left_buf, LEFT_OVERLAY_CONTENT, border_hl)
  update_overlay_content(right_buf, RIGHT_OVERLAY_CONTENT, border_hl)

  position_overlay_window('left_win', left_buf, LEFT_OVERLAY_WIDTH, combo_header_row, list_config.col)
  position_overlay_window(
    'right_win',
    right_buf,
    RIGHT_OVERLAY_WIDTH,
    combo_header_row,
    list_config.col + list_config.width
  )
end

local function clear_overlays_internal()
  if overlay_state.left_win and vim.api.nvim_win_is_valid(overlay_state.left_win) then
    vim.api.nvim_win_close(overlay_state.left_win, true)
    overlay_state.left_win = nil
  end

  if overlay_state.right_win and vim.api.nvim_win_is_valid(overlay_state.right_win) then
    vim.api.nvim_win_close(overlay_state.right_win, true)
    overlay_state.right_win = nil
  end

  overlay_state.last_row = nil
  overlay_state.last_col = nil
  overlay_state.last_border_hl = nil
  -- Note: we intentionally don't clear was_rendered here to track the transition
end

function M.detect_and_prepare(items, file_picker, win_width, combo_boost_score_multiplier, disable_combo_display)
  local combo_item_index, combo_count = detect_combo_item(items, file_picker, combo_boost_score_multiplier)

  if not combo_item_index then return false, nil, 0, nil end

  local header_line, text_len = create_header_text(combo_count, win_width, disable_combo_display)
  return true, header_line, text_len, combo_item_index
end

--- Render combo highlights and overlays
--- @return boolean was_hidden True if combo was just hidden (was rendered before, not now)
function M.render_highlights_and_overlays(
  combo_item_index,
  text_len,
  list_buf,
  list_win,
  ns_id,
  border_hl,
  item_to_lines,
  prompt_position,
  total_items
)
  local was_rendered_before = overlay_state.was_rendered
  local is_rendering_now = false

  if not combo_item_index then
    clear_overlays_internal()
  else
    local combo_item_lines = item_to_lines[combo_item_index]
    if not combo_item_lines then
      clear_overlays_internal()
    else
      local combo_header_line_idx = combo_item_lines.first
      apply_header_highlights(list_buf, ns_id, combo_header_line_idx, text_len, border_hl)
      if prompt_position == 'bottom' and total_items and total_items > 1 then
        combo_header_line_idx = combo_header_line_idx - 1
      end

      -- when rendering items in the reverse order for some reason this makes the
      -- indexing shifted by one in the internal list config, so just adjust for that
      update_overlays(list_win, combo_header_line_idx, border_hl)
      is_rendering_now = true
    end
  end

  overlay_state.was_rendered = is_rendering_now

  -- Return true if combo was just hidden (transition from visible to hidden)
  return was_rendered_before and not is_rendering_now
end

--- Get the combo header text for a given item
--- @param combo_count number The combo multiplier count
--- @param win_width number Window width for formatting
--- @param disable_combo_display boolean Whether to show combo count
--- @return string header_text The formatted header line
--- @return number text_len Length of the header text (without padding)
function M.get_combo_header_text(combo_count, win_width, disable_combo_display)
  return create_header_text(combo_count, win_width, disable_combo_display)
end

function M.get_overlay_widths() return LEFT_OVERLAY_WIDTH, RIGHT_OVERLAY_WIDTH end

function M.cleanup()
  clear_overlays_internal()
  overlay_state.was_rendered = false
end

return M
