local utils = require('fff.utils')

local M = {}

local function get_main_config()
  local main = require('fff.main')
  return main.config
end

local active_placements = {} ---@type table<number, any>
local loading_jobs = {} ---@type table<number, {metadata_job?: any}>

local function reserve_image_buffer_space(bufnr, metadata_lines_count)
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then return end

  local win = vim.fn.bufwinid(bufnr)
  local buffer_height = win ~= -1 and vim.api.nvim_win_get_height(win) or 24
  local lines_for_image = math.max(buffer_height - metadata_lines_count - 2, 5)

  local buffer_lines = {}
  for _ = 1, metadata_lines_count do
    table.insert(buffer_lines, '')
  end
  for _ = 1, lines_for_image do
    table.insert(buffer_lines, '')
  end

  local was_modifiable = vim.api.nvim_buf_get_option(bufnr, 'modifiable')
  vim.api.nvim_buf_set_option(bufnr, 'modifiable', true)

  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, buffer_lines)
  vim.api.nvim_buf_set_option(bufnr, 'modifiable', was_modifiable)

  return metadata_lines_count or 2
end

local function update_metadata_lines(bufnr, info_lines, reserved_lines_count)
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then return end

  reserved_lines_count = reserved_lines_count or 2

  local metadata_lines = {}
  for i = 1, reserved_lines_count do
    metadata_lines[i] = info_lines[i] or ''
  end

  vim.api.nvim_buf_set_option(bufnr, 'modifiable', true)
  vim.api.nvim_buf_set_lines(bufnr, 0, reserved_lines_count, false, metadata_lines)
  vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
end

local function identify_image_lines_async(file_path, bufnr, callback)
  local stat = vim.uv.fs_stat(file_path)
  local size_str = stat and utils.format_file_size(stat.size) or 'Unknown'

  local initial_info_lines = { ' Size: ' .. size_str }

  if vim.fn.executable('identify') == 0 then
    callback(initial_info_lines)
    return
  end

  callback(initial_info_lines)

  local config = get_main_config()
  local format_str = config and config.preview and config.preview.imagemagick_info_format_str
    or '%m: %wx%h, %[colorspace], %q-bit'

  local cmd = { 'identify', '-format', format_str, file_path }

  -- Cancel any previous metadata job
  if loading_jobs[bufnr] and loading_jobs[bufnr].metadata_job then
    pcall(loading_jobs[bufnr].metadata_job.kill, loading_jobs[bufnr].metadata_job, 9)
  end
  loading_jobs[bufnr] = loading_jobs[bufnr] or {}

  loading_jobs[bufnr].metadata_job = vim.system(cmd, { text = true }, function(result)
    vim.schedule(function()
      if not vim.api.nvim_buf_is_valid(bufnr) then return end

      local enhanced_info_lines = vim.deepcopy(initial_info_lines)
      if result.code == 0 and result.stdout and result.stdout ~= '' then
        local magick_info = ' ' .. result.stdout:gsub('\n', '')
        table.insert(enhanced_info_lines, magick_info)
      end

      callback(enhanced_info_lines)
    end)
  end)
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

function M.clear_buffer_images(bufnr)
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then return end

  if loading_jobs[bufnr] then
    if loading_jobs[bufnr].metadata_job then
      pcall(loading_jobs[bufnr].metadata_job.kill, loading_jobs[bufnr].metadata_job, 9)
    end
    loading_jobs[bufnr] = nil
  end

  if active_placements[bufnr] then
    pcall(active_placements[bufnr].close, active_placements[bufnr])
    active_placements[bufnr] = nil
  end

  local ok, snacks = pcall(require, 'snacks')
  if ok and snacks.image and snacks.image.placement then pcall(snacks.image.placement.clean, bufnr) end

  pcall(vim.api.nvim_buf_clear_namespace, bufnr, -1, 0, -1)
end

--- Load metadata of the image, displays it and display image in paralallel
--- Fully asynchronous
--- @param file_path string Path to the image file
--- @param bufnr number Buffer number to display in
--- @param max_width number Maximum width in characters
--- @param max_height number Maximum height in characters
--- @return boolean
function M.display_image(file_path, bufnr, max_width, max_height)
  max_width = max_width or 80
  max_height = max_height or 24
  vim.api.nvim_buf_set_option(bufnr, 'number', false)

  local reserved_metadata_lines = reserve_image_buffer_space(bufnr, 2)
  local image_content_starts_at_line = reserved_metadata_lines + 1
  identify_image_lines_async(
    file_path,
    bufnr,
    function(final_info_lines) update_metadata_lines(bufnr, final_info_lines, reserved_metadata_lines) end
  )

  local ok, snacks = pcall(require, 'snacks')
  if not ok then
    local error_lines = {
      '⚠ Image Preview Unavailable',
      '',
      'snacks.nvim plugin is not installed or not available.',
    }
    vim.api.nvim_buf_set_option(bufnr, 'modifiable', true)
    vim.api.nvim_buf_set_lines(bufnr, image_content_starts_at_line, -1, false, error_lines)
    vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
    return false
  end

  if not snacks.image.supports_terminal() then
    local error_lines = {
      '⚠ Image Preview Unavailable',
      '',
      'Terminal does not support image preview.',
      'Please use a terminal that supports images, such as Kitty, Wezterm or Alacritty.',
    }
    vim.api.nvim_buf_set_option(bufnr, 'modifiable', true)
    vim.api.nvim_buf_set_lines(bufnr, image_content_starts_at_line, -1, false, error_lines)
    vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
    return false
  end

  if not snacks.image.supports_file(file_path) then
    local error_lines = {
      '⚠ Unsupported Image Format',
      '',
      'File format is not supported for image preview.',
      'File: ' .. vim.fn.fnamemodify(file_path, ':t'),
    }
    vim.api.nvim_buf_set_option(bufnr, 'modifiable', true)
    vim.api.nvim_buf_set_lines(bufnr, image_content_starts_at_line, -1, false, error_lines)
    vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
    return false
  end

  if snacks.image and snacks.image.placement then
    M.clear_buffer_images(bufnr)

    vim.schedule(function()
      local success, placement = pcall(snacks.image.placement.new, bufnr, file_path, {
        pos = { image_content_starts_at_line, 1 },
        inline = true,
        fit = 'contain',
        auto_resize = true,
      })

      if success and placement then active_placements[bufnr] = placement end
    end)

    identify_image_lines_async(
      file_path,
      bufnr,
      function(final_info_lines) update_metadata_lines(bufnr, final_info_lines, reserved_metadata_lines) end
    )

    return true
  end

  return false
end

return M
