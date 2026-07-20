local M = {}

function M.calculate(total, cursor, height, prompt_position)
  height = math.max(1, height or 1)
  if total <= 0 then
    return {
      first = 1,
      last = 0,
      padding = prompt_position == 'bottom' and height or 0,
      cursor_line = 0,
    }
  end

  cursor = math.max(1, math.min(cursor or 1, total))
  local first = math.max(1, cursor - height + 1)
  local last = math.min(total, first + height - 1)
  local visible = last - first + 1
  local padding = prompt_position == 'bottom' and math.max(0, height - visible) or 0

  return {
    first = first,
    last = last,
    padding = padding,
    cursor_line = padding + cursor - first + 1,
  }
end

return M
