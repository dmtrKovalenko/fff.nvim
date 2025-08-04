local utils = require('fff.utils')

local M = {}

local function get_main_config()
  local main = require('fff.main')
  return main.config
end

local active_placements = {} ---@type table<number, any>

local function identify_image_lines(file_path)
  local stat = vim.uv.fs_stat(file_path)
  local size_str = stat and utils.format_file_size(stat.size) or 'Unknown'

  local info_lines = {}
  table.insert(info_lines, ' Size: ' .. size_str)

  local config = get_main_config()
  local format_str = config and config.preview and config.preview.imagemagick_info_format_str
    or '%m: %wx%h, %[colorspace], %q-bit'
  local cmd = string.format('identify -format "%s" "%s" 2>/dev/null', format_str, file_path)
  local magick_info = vim.fn.system(cmd)

  if vim.v.shell_error == 0 and magick_info and magick_info ~= '' then
    magick_info = ' ' .. magick_info:gsub('\n', '')
    table.insert(info_lines, magick_info)
  end

  return info_lines
end

-- This is a required function for snacks nvim to fill the buffer with enough space
local function fill_buffer_space_for_image_preview(bufnr, info_lines)
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then return end

  local win = vim.fn.bufwinid(bufnr)
  local buffer_height = win ~= -1 and vim.api.nvim_win_get_height(win) or 24
  local lines_for_image = math.max(buffer_height - #info_lines - 2, 5)

  local buffer_lines = vim.list_extend({}, info_lines)
  for _ = 1, lines_for_image do
    table.insert(buffer_lines, '')
  end

  local was_modifiable = vim.api.nvim_buf_get_option(bufnr, 'modifiable')
  vim.api.nvim_buf_set_option(bufnr, 'modifiable', true)

  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, buffer_lines)

  vim.api.nvim_buf_set_option(bufnr, 'modifiable', was_modifiable)
end

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

--- Clear any existing image attachments from buffer
--- @param bufnr number Buffer number
function M.clear_buffer_images(bufnr)
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then return end

  if active_placements[bufnr] then
    pcall(active_placements[bufnr].close, active_placements[bufnr])
    active_placements[bufnr] = nil
  end

  local ok, snacks = pcall(require, 'snacks')
  if ok and snacks.image and snacks.image.placement then pcall(snacks.image.placement.clean, bufnr) end
end

--- Display image using the simplest approach that works
--- @param file_path string Path to the image file
--- @param bufnr number Buffer number to display in
--- @param max_width number Maximum width in characters
--- @param max_height number Maximum height in characters
function M.display_image(file_path, bufnr, max_width, max_height)
  max_width = max_width or 80
  max_height = max_height or 24

  local ok, snacks = pcall(require, 'snacks')
  if ok and snacks.image and snacks.image.buf then
    M.clear_buffer_images(bufnr)

    local info_lines = identify_image_lines(file_path)

    fill_buffer_space_for_image_preview(bufnr, info_lines)
    vim.schedule(function()
      local success, placement = pcall(snacks.image.placement.new, bufnr, file_path, {
        pos = { #info_lines + 1, 1 },
        inline = true,
        fit = 'contain',
        auto_resize = true,
      })

      if success and placement then
        active_placements[bufnr] = placement
      else
        M.display_image_info(file_path, bufnr, 'Snacks.nvim failed: ' .. tostring(placement or 'unknown error'))
      end
    end)
    return
  end

  M.display_image_info(file_path, bufnr, 'Snacks.nvim not available')
end

--- Display image information when image display fails
--- @param file_path string Path to the image file
--- @param bufnr number Buffer number to display in
--- @param reason string|nil Reason for failure
function M.display_image_info(file_path, bufnr, reason)
  local info_lines = identify_image_lines(file_path)

  if reason then
    table.insert(info_lines, '')
    table.insert(info_lines, string.rep('─', 50))

    table.insert(info_lines, ' Preview is not available')
    table.insert(info_lines, ' Reason: ' .. reason)
  end

  fill_buffer_space_for_image_preview(bufnr, info_lines)
end

return M
