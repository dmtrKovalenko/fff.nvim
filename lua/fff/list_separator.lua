-- Renders list separator at any index, designed to be floating on top of list renderer
local M = {}

local LEFT_PADDING = 2
local RIGHT_PADDING = 1
-- overflow BOTH borders on left and right
local OVERFLOW_TOTAL = 2

---@class fff.list_separator.State
---@field buf integer|nil
---@field win integer|nil
---@field ns_id integer
---@field last string|nil
local state = {
  buf = nil,
  win = nil,
  ns_id = 0,
  last = nil,
}

--- @class FffSeparatorOpts
--- @field list_win number List window handle (used to read its config)
--- @field row number 1-based screen row where the separator sits (relative to editor)
--- @field text string Label text rendered between the dashes (callers prefix arrow glyphs themselves)
--- @field text_hl string Highlight group for the label
--- @field border_hl string Highlight group for the dashes / junctions

function M.init(ns_id) state.ns_id = ns_id end

---@return integer
local function get_or_create_buf()
  if not state.buf or not vim.api.nvim_buf_is_valid(state.buf) then
    state.buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_set_option_value('bufhidden', 'wipe', { buf = state.buf })
  end
  return state.buf --[[@as integer]]
end

--- @param total_width number Width of the float in cells (incl. `├` and `┤`)
--- @param text string Label text (we add the surrounding spaces)
--- @return string content
--- @return number label_byte_start Byte offset where the highlighted label begins
--- @return number label_byte_len Byte length of the highlighted label run
local function build_line(total_width, text)
  local label = ' ' .. text .. ' '
  local label_disp = vim.fn.strdisplaywidth(label)
  -- 2 cells consumed by `├` and `┤`, plus LEFT_PADDING + RIGHT_PADDING dashes.
  local inner = math.max(0, total_width - 2 - LEFT_PADDING - RIGHT_PADDING - label_disp)
  local left = string.rep('─', LEFT_PADDING)
  local right = string.rep('─', inner + RIGHT_PADDING)
  local content = '├' .. left .. label .. right .. '┤'
  local label_byte_start = #('├' .. left)
  return content, label_byte_start, #label
end

--- Render or reposition the separator
--- @param opts FffSeparatorOpts
function M.update(opts)
  local list_cfg = vim.api.nvim_win_get_config(opts.list_win)
  local list_col = list_cfg.col
  local list_width = list_cfg.width

  -- Span the full bordered list footprint: `├` lands on the left vertical,
  -- `┤` on the right. nvim_win_get_config().col is already the column of the
  -- left border, so we don't shift further.
  local total_width = list_width + OVERFLOW_TOTAL
  local col = list_col
  local row = opts.row

  local key = string.format('%d|%d|%d|%s|%s|%s', row, col, total_width, opts.text, opts.text_hl, opts.border_hl)
  if state.last == key and state.win and vim.api.nvim_win_is_valid(state.win) then return end

  local buf = get_or_create_buf()
  local content, label_byte_start, label_byte_len = build_line(total_width, opts.text)

  vim.api.nvim_set_option_value('modifiable', true, { buf = buf })
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, { content })
  vim.api.nvim_buf_clear_namespace(buf, state.ns_id, 0, -1)
  vim.api.nvim_buf_set_extmark(buf, state.ns_id, 0, 0, {
    end_row = 1,
    end_col = 0,
    hl_group = opts.border_hl,
    hl_eol = false,
  })
  if label_byte_len > 0 then
    vim.api.nvim_buf_set_extmark(buf, state.ns_id, 0, label_byte_start, {
      end_row = 0,
      end_col = label_byte_start + label_byte_len,
      hl_group = opts.text_hl,
    })
  end
  vim.api.nvim_set_option_value('modifiable', false, { buf = buf })

  -- the actual fake floating window
  local win_cfg = {
    relative = 'editor',
    width = total_width,
    height = 1,
    row = row,
    col = col,
    style = 'minimal',
    border = 'none',
    focusable = false,
    zindex = 250,
  }

  if state.win and vim.api.nvim_win_is_valid(state.win) then
    vim.api.nvim_win_set_config(state.win, win_cfg)
  else
    state.win = vim.api.nvim_open_win(buf, false, win_cfg)
    vim.api.nvim_set_option_value('winhighlight', 'Normal:Normal', { win = state.win })
  end

  state.last = key
end

--- Hide the separator if visible. Safe to call repeatedly.
--- @return boolean was_visible True if a hide actually happened
function M.hide()
  local was_visible = false
  if state.win and vim.api.nvim_win_is_valid(state.win) then
    pcall(vim.api.nvim_win_close, state.win, true)
    was_visible = true
  end
  state.win = nil
  state.last = nil
  return was_visible
end

function M.cleanup()
  M.hide()
  if state.buf and vim.api.nvim_buf_is_valid(state.buf) then
    pcall(vim.api.nvim_buf_delete, state.buf, { force = true })
  end
  state.buf = nil
end

return M
