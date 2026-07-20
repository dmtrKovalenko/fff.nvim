local M = {}

local function resolve(value, fallback, columns, lines)
  if type(value) == 'function' then return value(columns, lines) end
  return value or fallback
end

function M.frame(columns, lines, config, fullscreen)
  if fullscreen then
    return {
      width = math.max(1, columns - 4),
      height = math.max(1, lines - 4),
      col = 1,
      row = 0,
    }
  end

  local width_ratio = resolve(config.width, 0.8, columns, lines)
  local height_ratio = resolve(config.height, 0.8, columns, lines)
  local width = math.max(1, math.floor(columns * width_ratio))
  local height = math.max(1, math.floor(lines * height_ratio))

  return {
    width = width,
    height = height,
    col = math.floor((columns - width) / 2),
    row = math.floor((lines - height) / 2),
  }
end

return M
