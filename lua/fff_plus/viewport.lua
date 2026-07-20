local M = {}

function M.calculate(total, cursor, height, prompt_position)
  height = math.max(1, height or 1)
  local reverse = prompt_position == 'bottom'
  if total <= 0 then
    return {
      first = 1,
      last = 0,
      padding = reverse and height or 0,
      cursor_line = 0,
      reverse = reverse,
    }
  end

  cursor = math.max(1, math.min(cursor or 1, total))
  local first = math.max(1, cursor - height + 1)
  local last = math.min(total, first + height - 1)
  local visible = last - first + 1
  local padding = reverse and math.max(0, height - visible) or 0
  local cursor_line = reverse and (padding + last - cursor + 1) or (cursor - first + 1)

  return {
    first = first,
    last = last,
    padding = padding,
    cursor_line = cursor_line,
    reverse = reverse,
  }
end

function M.move(cursor, total, direction, prompt_position)
  if total <= 0 then return 1 end
  local delta = direction == 'up' and -1 or 1
  if prompt_position == 'bottom' then delta = -delta end
  return math.max(1, math.min(cursor + delta, total))
end

return M
