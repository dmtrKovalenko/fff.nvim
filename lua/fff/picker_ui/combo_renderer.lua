--- Combo header policy.
---
--- Pure logic: detects whether the current result set has a "combo" boost,
--- and produces the label text for the list separator. The actual divider
--- is rendered by `list_separator` so this module owns no windows or buffers.
local M = {}

local COMBO_TEXT_FORMAT = 'Last Match (×%d combo)'
local LAST_MATCH_TEXT = 'Last Match'

--- @param items table[]
--- @param file_picker table
--- @param combo_boost_score_multiplier number
--- @return number|nil idx 1-based item index of the combo anchor, nil if none
--- @return number combo_count Multiplier (combo_match_boost / multiplier)
local function detect_combo_item(items, file_picker, combo_boost_score_multiplier)
  if not items or #items == 0 then return nil, 0 end

  local first_score = file_picker.get_file_score(1)
  local last_score = file_picker.get_file_score(#items)

  if first_score and first_score.combo_match_boost > combo_boost_score_multiplier then
    return 1, first_score.combo_match_boost / combo_boost_score_multiplier
  elseif last_score and last_score.combo_match_boost > combo_boost_score_multiplier then
    return #items, last_score.combo_match_boost / combo_boost_score_multiplier
  end

  return nil, 0
end

--- @class FffComboInfo
--- @field idx number Item index of the combo anchor
--- @field text string Label text for the separator
--- @field count number Combo multiplier

--- @param items table[]
--- @param file_picker table
--- @param combo_boost_score_multiplier number
--- @param disable_combo_display boolean
--- @return FffComboInfo|nil
function M.detect(items, file_picker, combo_boost_score_multiplier, disable_combo_display)
  local idx, count = detect_combo_item(items, file_picker, combo_boost_score_multiplier)
  if not idx then return nil end

  local text = disable_combo_display and LAST_MATCH_TEXT or string.format(COMBO_TEXT_FORMAT, count)
  return { idx = idx, text = text, count = count }
end

return M
