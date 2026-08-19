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

local function window_config(width, height, col, row)
  return {
    relative = 'editor',
    width = math.max(1, width),
    height = math.max(1, height),
    col = col,
    row = row,
    border = 'single',
    style = 'minimal',
  }
end

function M.windows(frame, config, preview_requested)
  config = config or {}
  local position = config.preview_position or 'right'
  local horizontal = position == 'left' or position == 'right'
  local preview_size = resolve(config.preview_size, 0.5, frame.width, frame.height)
  local enough_space = horizontal and frame.width >= (config.preview_min_width or 50)
    or (not horizontal and frame.height >= (config.preview_min_height or 12))
  local has_preview = preview_requested and enough_space
  local main = { width = frame.width, height = frame.height, col = frame.col, row = frame.row }
  local preview_frame

  if has_preview and horizontal then
    local preview_width = math.max(1, math.floor(frame.width * preview_size))
    main.width = math.max(1, frame.width - preview_width - 2)
    preview_frame = { width = preview_width, height = frame.height, row = frame.row }
    if position == 'left' then
      preview_frame.col = frame.col
      main.col = frame.col + preview_width + 2
    else
      preview_frame.col = frame.col + main.width + 2
    end
  elseif has_preview then
    local preview_height = math.max(1, math.floor(frame.height * preview_size))
    main.height = math.max(4, frame.height - preview_height - 2)
    preview_frame = { width = frame.width, height = preview_height, col = frame.col }
    if position == 'top' then
      preview_frame.row = frame.row
      main.row = frame.row + preview_height + 2
    else
      preview_frame.row = frame.row + main.height + 2
    end
  end

  local prompt_position = config.prompt_position or 'bottom'
  local list_height = math.max(1, main.height - 3)
  local list_row = prompt_position == 'bottom' and main.row or main.row + 2
  local input_row = prompt_position == 'bottom' and main.row + list_height + 2 or main.row
  local windows = {
    has_preview = has_preview,
    list = window_config(main.width, list_height, main.col, list_row),
    input = window_config(main.width, 1, main.col, input_row),
  }
  if preview_frame then
    windows.preview = window_config(preview_frame.width, preview_frame.height, preview_frame.col, preview_frame.row)
  end
  return windows
end

return M
