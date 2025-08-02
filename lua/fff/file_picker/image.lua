--- Image handling module for file picker
--- Simple implementation that delegates to Snacks.nvim

local M = {}

-- Track active image placements per buffer
local active_placements = {} ---@type table<number, any>

-- Helper function to safely set buffer lines
local function safe_set_buffer_lines(bufnr, start, end_line, strict_indexing, lines)
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then return false end

  -- Make buffer modifiable temporarily
  local was_modifiable = vim.api.nvim_buf_get_option(bufnr, 'modifiable')
  vim.api.nvim_buf_set_option(bufnr, 'modifiable', true)

  -- Set the lines
  local ok, err = pcall(vim.api.nvim_buf_set_lines, bufnr, start, end_line, strict_indexing, lines)

  -- Restore modifiable state
  vim.api.nvim_buf_set_option(bufnr, 'modifiable', was_modifiable)

  return ok
end

-- Common image extensions (SVG excluded for text preview)
local IMAGE_EXTENSIONS = {
  '.jpg',
  '.jpeg',
  '.png',
  '.gif',
  '.bmp',
  '.tiff',
  '.tif',
  '.webp',
  '.ico',
  '.pdf',
  '.ps',
  '.eps',
  '.heic',
  '.avif',
}

--- Check if file is an image
--- @param file_path string Path to the file
--- @return boolean True if file is an image
function M.is_image(file_path)
  local ext = string.lower(vim.fn.fnamemodify(file_path, ':e'))
  if ext == '' then return false end

  for _, image_ext in ipairs(IMAGE_EXTENSIONS) do
    if '.' .. ext == image_ext then return true end
  end

  return false
end

--- Check if we're in Kitty terminal
--- @return boolean True if in Kitty
function M.is_kitty() return vim.env.KITTY_PID ~= nil end

--- Clear any existing image attachments from buffer
--- @param bufnr number Buffer number
function M.clear_buffer_images(bufnr)
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then return end

  -- Close any tracked placement for this buffer
  if active_placements[bufnr] then
    pcall(active_placements[bufnr].close, active_placements[bufnr])
    active_placements[bufnr] = nil
  end

  local ok, snacks = pcall(require, 'snacks')
  if ok and snacks.image and snacks.image.placement then
    -- Use the proper Snacks.nvim placement cleanup API
    -- This removes all image placements for the specified buffer
    pcall(snacks.image.placement.clean, bufnr)
  end
end

--- Display image using the simplest approach that works
--- @param file_path string Path to the image file
--- @param bufnr number Buffer number to display in
--- @param max_width number Maximum width in characters
--- @param max_height number Maximum height in characters
function M.display_image(file_path, bufnr, max_width, max_height)
  max_width = max_width or 80
  max_height = max_height or 24

  if not M.is_kitty() then
    M.display_image_info(file_path, bufnr, 'Not in Kitty terminal')
    return
  end

  -- Try Snacks.nvim first (most reliable)
  local ok, snacks = pcall(require, 'snacks')
  if ok and snacks.image and snacks.image.buf then
    -- Clear any existing image attachments first
    M.clear_buffer_images(bufnr)

    -- Clear buffer content to prevent text/image overlap
    safe_set_buffer_lines(bufnr, 0, -1, false, {})

    -- Configure Snacks image to prevent repetition
    local success, placement = pcall(snacks.image.buf.attach, bufnr, {
      src = file_path,
      fit = 'contain', -- Fit image within bounds without repetition
    })

    if success and placement then
      -- Track the placement so we can clean it up later
      active_placements[bufnr] = placement
      return -- Successfully attached image, we're done
    else
      M.display_image_info(file_path, bufnr, 'Snacks.nvim failed: ' .. tostring(placement or 'unknown error'))
      return
    end
  end

  M.display_image_info(file_path, bufnr, 'Snacks.nvim not available')
end

--- Display image information when image display fails
--- @param file_path string Path to the image file
--- @param bufnr number Buffer number to display in
--- @param reason string|nil Reason for failure
function M.display_image_info(file_path, bufnr, reason)
  local info = {}

  local stat = vim.uv.fs_stat(file_path)
  if stat then
    table.insert(info, string.format('📁 File: %s', vim.fn.fnamemodify(file_path, ':t')))
    table.insert(info, string.format('📏 Size: %d bytes', stat.size))
    table.insert(info, string.format('🕒 Modified: %s', os.date('%Y-%m-%d %H:%M:%S', stat.mtime.sec)))
  end

  local width, height = M.get_image_dimensions(file_path)
  if width and height then table.insert(info, string.format('🖼️  Dimensions: %dx%d pixels', width, height)) end

  local ext = string.lower(vim.fn.fnamemodify(file_path, ':e'))
  if ext ~= '' then table.insert(info, string.format('🎨 Format: %s', ext:upper())) end

  table.insert(info, '')
  table.insert(info, '┌─ Image Preview Debug ─┐')
  table.insert(info, string.format('│ Kitty: %s          │', M.is_kitty() and 'Yes' or 'No'))
  table.insert(info, string.format('│ KITTY_PID: %s      │', vim.env.KITTY_PID or 'nil'))
  if reason then
    table.insert(info, string.format('│ Issue: %s', reason:sub(1, 16)))
    if #reason > 16 then table.insert(info, string.format('│        %s', reason:sub(17, 32))) end
  end
  table.insert(info, '└─────────────────────────┘')

  safe_set_buffer_lines(bufnr, 0, -1, false, info)
end

--- Get image dimensions using file command
--- @param file_path string Path to the image file
--- @return number|nil, number|nil Width and height in pixels
function M.get_image_dimensions(file_path)
  -- Try file command first
  local cmd = string.format('file "%s"', file_path)
  local result = vim.fn.system(cmd)

  if vim.v.shell_error == 0 then
    local width, height = result:match('(%d+)%s*x%s*(%d+)')
    if width and height then return tonumber(width), tonumber(height) end
  end

  -- Fallback to identify command (ImageMagick)
  cmd = string.format('identify -format "%%w %%h" "%s" 2>/dev/null', file_path)
  result = vim.fn.system(cmd)

  if vim.v.shell_error == 0 then
    local width, height = result:match('(%d+)%s+(%d+)')
    if width and height then return tonumber(width), tonumber(height) end
  end

  return nil, nil
end

return M
