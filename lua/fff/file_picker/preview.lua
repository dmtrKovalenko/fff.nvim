local utils = require('fff.utils')
local file_picker = require('fff.file_picker')

local M = {}

local image = nil
local function get_image()
  if not image then image = require('fff.file_picker.image') end
  return image
end

-- Set lines in preview buffer (simpler approach)
local function set_buffer_lines(bufnr, lines)
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then return end

  vim.api.nvim_buf_set_option(bufnr, 'modifiable', true)
  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, lines)
  vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
end

-- Check if file is already loaded in a buffer
local function find_existing_buffer(file_path)
  local abs_path = vim.fn.resolve(vim.fn.fnamemodify(file_path, ':p'))

  for _, bufnr in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(bufnr) then
      local buf_name = vim.api.nvim_buf_get_name(bufnr)
      if buf_name ~= '' then
        local buf_path = vim.fn.resolve(vim.fn.fnamemodify(buf_name, ':p'))
        if buf_path == abs_path then return bufnr end
      end
    end
  end
  return nil
end

local function read_file_fast(file_path, max_lines)
  local fd = vim.uv.fs_open(file_path, 'r', 0x666)
  if not fd then return nil end

  local stat = vim.uv.fs_fstat(fd)
  if not stat then
    vim.uv.fs_close(fd)
    return nil
  end

  local data = vim.uv.fs_read(fd, stat.size, 0)
  vim.uv.fs_close(fd)

  if not data then return nil end

  local lines = vim.split(data, '\n', { plain = true })
  if max_lines and #lines > max_lines then
    local limited_lines = {}
    for i = 1, max_lines do
      table.insert(limited_lines, lines[i])
    end
    lines = limited_lines
  end

  return lines
end

local function copy_buffer_content(source_bufnr, target_bufnr)
  local lines = vim.api.nvim_buf_get_lines(source_bufnr, 0, -1, false)
  set_buffer_lines(target_bufnr, lines)

  local source_ft = vim.api.nvim_buf_get_option(source_bufnr, 'filetype')
  if source_ft ~= '' then vim.api.nvim_buf_set_option(target_bufnr, 'filetype', source_ft) end

  return true
end

-- Config will be set from main.lua
M.config = nil

M.state = {
  bufnr = nil,
  winid = nil,
  current_file = nil,
  scroll_offset = 0,
  content_height = 0,
}

--- Setup preview configuration
--- @param config table Configuration options
function M.setup(config) M.config = config or {} end

--- Check if file is binary
--- @param file_path string Path to the file
--- @return boolean True if file appears to be binary
function M.is_binary_file(file_path)
  local ext = string.lower(vim.fn.fnamemodify(file_path, ':e'))
  local binary_extensions = {
    'jpg',
    'jpeg',
    'png',
    'gif',
    'bmp',
    'tiff',
    'tif',
    'webp',
    'ico',
    'pdf',
    'ps',
    'eps',
    'heic',
    'avif',
    -- Archives
    'zip',
    'rar',
    '7z',
    'tar',
    'gz',
    'bz2',
    'xz',
    -- Executables
    'exe',
    'dll',
    'so',
    'dylib',
    'bin',
    -- Audio/Video
    'mp3',
    'mp4',
    'avi',
    'mkv',
    'wav',
    'flac',
    'ogg',
    -- Other binary formats
    'db',
    'sqlite',
    'dat',
    'bin',
    'iso',
  }

  for _, binary_ext in ipairs(binary_extensions) do
    if ext == binary_ext then return true end
  end

  local file = io.open(file_path, 'rb')
  if not file then return false end

  local chunk = file:read(M.config.binary_file_threshold)
  file:close()

  if not chunk then return false end
  if chunk:find('\0') then return true end

  local printable_count = 0
  local total_count = #chunk

  for i = 1, total_count do
    local byte = chunk:byte(i)
    -- Printable ASCII range + common control chars (tab, newline, carriage return)
    if (byte >= 32 and byte <= 126) or byte == 9 or byte == 10 or byte == 13 then
      printable_count = printable_count + 1
    end
  end

  local printable_ratio = printable_count / total_count
  return printable_ratio < 0.8 -- More aggressive: If less than 80% printable, consider binary
end

--- Get file information
--- @param file_path string Path to the file
--- @return table | nil File information
function M.get_file_info(file_path)
  local stat = vim.uv.fs_stat(file_path)
  if not stat then return nil end

  local info = {
    name = vim.fn.fnamemodify(file_path, ':t'),
    path = file_path,
    size = stat.size,
    modified = stat.mtime.sec,
    accessed = stat.atime.sec,
    type = stat.type,
  }

  info.extension = vim.fn.fnamemodify(file_path, ':e'):lower()
  info.filetype = vim.filetype.match({ filename = file_path }) or 'text'
  info.size_formatted = utils.format_file_size(info.size)
  info.modified_formatted = os.date('%Y-%m-%d %H:%M:%S', info.modified)
  info.accessed_formatted = os.date('%Y-%m-%d %H:%M:%S', info.accessed)

  return info
end

--- Create file info content without custom borders
--- @param file table File information from search results
--- @param info table File system information
--- @param file_index number Index of the file in search results (for score lookup)
--- @return table Lines for the file info content
function M.create_file_info_content(file, info, file_index)
  local lines = {}

  local score = file_index and file_picker.get_file_score(file_index) or nil
  table.insert(
    lines,
    string.format('Size: %-8s │ Total Score: %d', info.size_formatted or 'N/A', score and score.total or 0)
  )
  table.insert(
    lines,
    string.format('Type: %-8s │ Match Type: %s', info.filetype or 'text', score and score.match_type or 'unknown')
  )
  table.insert(
    lines,
    string.format(
      'Git:  %-8s │ Frecency Mod: %d, Acc: %d',
      file.git_status or 'clear',
      file.modification_frecency_score or 0,
      file.access_frecency_score or 0
    )
  )

  if score then
    table.insert(
      lines,
      string.format(
        'Score Breakdown: base=%d, name_bonus=%d, special_bonus=%d',
        score.base_score,
        score.filename_bonus,
        score.special_filename_bonus
      )
    )
    table.insert(
      lines,
      string.format('Score Modifiers: frec_boost=%d, dist_penalty=%d', score.frecency_boost, score.distance_penalty)
    )
  else
    table.insert(lines, 'Score Breakdown: N/A (no score data available)')
  end
  table.insert(lines, '')

  -- Time information section
  table.insert(lines, 'TIMINGS')
  table.insert(lines, string.rep('─', 50))
  table.insert(lines, string.format('Modified: %s', info.modified_formatted or 'N/A'))
  table.insert(lines, string.format('Last Access: %s', info.accessed_formatted or 'N/A'))

  return lines
end

--- Create file info header
--- @param info table File information
--- @return table Lines for the header
function M.create_file_info_header(info)
  if not M.config.show_file_info or not info then return {} end

  local header = {}
  table.insert(header, string.format('File: %s', info.name))
  table.insert(header, string.format('Size: %s', info.size_formatted))
  table.insert(header, string.format('Modified: %s', info.modified_formatted))
  table.insert(header, string.format('Type: %s', info.filetype))

  if info.extension ~= '' then table.insert(header, string.format('Extension: .%s', info.extension)) end

  table.insert(header, string.rep('─', 50))
  table.insert(header, '')

  return header
end

--- Read file content with proper handling
--- @param file_path string Path to the file
--- @param max_lines number Maximum lines to read
--- @return table|nil Lines of content, nil if failed
function M.read_file_content(file_path, max_lines)
  local file = io.open(file_path, 'r')
  if not file then return nil end

  local lines = {}
  local line_count = 0

  for line in file:lines() do
    line_count = line_count + 1

    -- Handle very long lines by truncating them
    if #line > 500 then line = line:sub(1, 497) .. '...' end

    table.insert(lines, line)

    if line_count >= max_lines then
      table.insert(lines, '')
      table.insert(lines, string.format('... (truncated, showing first %d lines)', max_lines))
      break
    end
  end

  file:close()
  return lines
end

--- Read file tail (last N lines)
--- @param file_path string Path to the file
--- @param tail_lines number Number of lines from the end
--- @return table|nil Lines of content, nil if failed
function M.read_file_tail(file_path, tail_lines)
  local cmd = string.format('tail -n %d %s 2>/dev/null', tail_lines, vim.fn.shellescape(file_path))
  local result = vim.fn.system(cmd)

  if vim.v.shell_error ~= 0 then return M.read_file_content(file_path, tail_lines) end

  local lines = vim.split(result, '\n')
  if lines[#lines] == '' then table.remove(lines) end

  return lines
end

--- Preview a regular file
--- @param file_path string Path to the file
--- @param bufnr number Buffer number for preview
--- @return boolean Success status
function M.preview_file(file_path, bufnr)
  local info = M.get_file_info(file_path)
  if not info then return false end

  if info.size > M.config.max_size then
    local lines = {
      'File too large for preview',
      string.format(
        'Size: %s (max: %s)',
        info.size_formatted,
        string.format('%.1fMB', M.config.max_size / 1024 / 1024)
      ),
      '',
      'Use a text editor to view this file.',
    }
    set_buffer_lines(bufnr, lines)
    return true
  end

  -- if the buffer is already opened for this file we reuse the buffer directly
  local existing_bufnr = find_existing_buffer(file_path)

  if existing_bufnr then
    local success = copy_buffer_content(existing_bufnr, bufnr)
    if success then
      local file_config = M.get_file_config(file_path)

      vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
      vim.api.nvim_buf_set_option(bufnr, 'readonly', true)
      vim.api.nvim_buf_set_option(bufnr, 'buftype', 'nofile')
      vim.api.nvim_buf_set_option(bufnr, 'wrap', file_config.wrap_lines or M.config.wrap_lines)
      vim.api.nvim_buf_set_option(bufnr, 'number', M.config.line_numbers)

      local content = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
      M.state.content_height = #content
      M.state.scroll_offset = 0

      return true
    end
  end

  -- Fallback: Manual file reading with fast UV operations
  local file_config = M.get_file_config(file_path)

  local content
  if file_config.tail_lines then
    content = M.read_file_tail(file_path, file_config.tail_lines)
  else
    content = read_file_fast(file_path, M.config.max_lines)
    if not content then content = M.read_file_content(file_path, M.config.max_lines) end
  end

  if not content then return false end

  set_buffer_lines(bufnr, content)

  vim.api.nvim_buf_set_option(bufnr, 'filetype', info.filetype)
  vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
  vim.api.nvim_buf_set_option(bufnr, 'readonly', true)
  vim.api.nvim_buf_set_option(bufnr, 'buftype', 'nofile')
  vim.api.nvim_buf_set_option(bufnr, 'wrap', file_config.wrap_lines or M.config.wrap_lines)
  vim.api.nvim_buf_set_option(bufnr, 'number', M.config.line_numbers)

  M.state.content_height = #content
  M.state.scroll_offset = 0

  return true
end

--- Preview a binary file
--- @param file_path string Path to the file
--- @param bufnr number Buffer number for preview
--- @return boolean Success status
function M.preview_binary_file(file_path, bufnr)
  local lines = {}
  local info = M.get_file_info(file_path)
  table.insert(lines, '⚠ Binary File Detected')
  table.insert(lines, '')
  table.insert(lines, 'This file contains binary data and cannot be displayed as text.')
  table.insert(lines, '')

  -- Try to get more information about the binary file
  if vim.fn.executable('file') == 1 then
    local cmd = string.format('file -b %s', vim.fn.shellescape(file_path))
    local result = vim.fn.system(cmd)
    if vim.v.shell_error == 0 and result then
      result = result:gsub('\n', '')
      table.insert(lines, 'File type: ' .. result)
      table.insert(lines, '')
    end
  end

  -- Show hex dump for small binary files
  if info.size <= 1024 and vim.fn.executable('xxd') == 1 then
    table.insert(lines, 'Hex dump (first 1KB):')
    table.insert(lines, '')

    local cmd = string.format('xxd -l 1024 %s', vim.fn.shellescape(file_path))
    local hex_result = vim.fn.system(cmd)
    if vim.v.shell_error == 0 and hex_result then
      local hex_lines = vim.split(hex_result, '\n')
      for _, line in ipairs(hex_lines) do
        if line:match('%S') then table.insert(lines, line) end
      end
    end
  else
    table.insert(lines, 'Use a hex editor or appropriate application to view this file.')
  end

  set_buffer_lines(bufnr, lines)
  vim.api.nvim_buf_set_option(bufnr, 'filetype', 'text')
  vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
  vim.api.nvim_buf_set_option(bufnr, 'readonly', true)

  return true
end

--- Get file-specific configuration
--- @param file_path string Path to the file
--- @return table Configuration for the file
function M.get_file_config(file_path)
  if not M.config or not M.config.filetypes then return {} end

  local filetype = vim.filetype.match({ filename = file_path }) or 'text'
  return M.config.filetypes[filetype] or {}
end

--- @param file_path string Path to the file or directory
--- @param bufnr number Buffer number for preview
--- @return boolean if the preview was successful
function M.preview(file_path, bufnr)
  if not file_path or file_path == '' then
    M.clear_buffer(bufnr)
    set_buffer_lines(bufnr, { 'No file selected' })
    return false
  end

  M.state.current_file = file_path
  M.state.bufnr = bufnr

  M.clear_buffer(bufnr)

  if get_image().is_image(file_path) then
    local win_width = 80
    local win_height = 24

    if M.state.winid and vim.api.nvim_win_is_valid(M.state.winid) then
      win_width = vim.api.nvim_win_get_width(M.state.winid) - 2
      win_height = vim.api.nvim_win_get_height(M.state.winid) - 2
    end

    return get_image().display_image(file_path, bufnr, win_width, win_height)
  elseif M.is_binary_file(file_path) then
    return M.preview_binary_file(file_path, bufnr)
  else
    return M.preview_file(file_path, bufnr)
  end
end

function M.scroll(lines)
  if not M.state.bufnr or not vim.api.nvim_buf_is_valid(M.state.bufnr) then return end
  if not M.state.winid or not vim.api.nvim_win_is_valid(M.state.winid) then return end

  local win_height = vim.api.nvim_win_get_height(M.state.winid)
  vim.notify(win_height)
  local content_height = M.state.content_height or 0

  -- allows scrolling for a full content + half window
  local half_screen = math.floor(win_height / 2)
  local max_scroll = math.max(0, content_height + half_screen - win_height)

  local current_offset = M.state.scroll_offset or 0
  local new_offset = math.max(0, math.min(max_scroll, current_offset + lines))

  if new_offset ~= current_offset then
    M.state.scroll_offset = new_offset

    local target_line = math.min(content_height, math.max(1, new_offset + 1))

    vim.api.nvim_win_call(M.state.winid, function()
      vim.api.nvim_win_set_cursor(M.state.winid, { target_line, 0 })
      vim.cmd('normal! zt')
    end)
  end
end

--- Set preview window
--- @param winid number Window ID for the preview
function M.set_preview_window(winid) M.state.winid = winid end

--- Create preview header with file information
--- @param file table File information from search results
--- @return table Lines for the preview header
function M.create_preview_header(file)
  if not file then return {} end

  local header = {}
  local filename = file.name or vim.fn.fnamemodify(file.path or '', ':t')
  local dir = file.directory or vim.fn.fnamemodify(file.path or '', ':h')
  if dir == '.' then dir = '' end

  -- Header with file info
  table.insert(header, string.format('📄 %s', filename))
  if dir ~= '' then table.insert(header, string.format('📁 %s', dir)) end
  table.insert(header, string.rep('─', 50))
  table.insert(header, '')

  return header
end

--- Update file info buffer
--- @param file table File information from search results
--- @param bufnr number Buffer number for file info
--- @return boolean Success status
function M.update_file_info_buffer(file, bufnr, file_index)
  if not file then
    set_buffer_lines(bufnr, { 'No file selected' })
    return false
  end

  local info = M.get_file_info(file.path)
  if not info then
    set_buffer_lines(bufnr, { 'File info unavailable' })
    return false
  end

  local file_info_lines = M.create_file_info_content(file, info, file_index)
  set_buffer_lines(bufnr, file_info_lines)

  vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
  vim.api.nvim_buf_set_option(bufnr, 'readonly', true)
  vim.api.nvim_buf_set_option(bufnr, 'buftype', 'nofile')
  vim.api.nvim_buf_set_option(bufnr, 'wrap', false)

  return true
end

--- Clear buffer completely including any image attachments
--- @param bufnr number Buffer number to clear
function M.clear_buffer(bufnr)
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then return end

  get_image().clear_buffer_images(bufnr)
  set_buffer_lines(bufnr, {})
  vim.api.nvim_buf_set_option(bufnr, 'filetype', '')
end

--- Clear preview
function M.clear()
  if M.state.bufnr and vim.api.nvim_buf_is_valid(M.state.bufnr) then
    M.clear_buffer(M.state.bufnr)
    set_buffer_lines(M.state.bufnr, { 'No preview available' })
  end

  M.state.current_file = nil
  M.state.scroll_offset = 0
  M.state.content_height = 0
end

return M
