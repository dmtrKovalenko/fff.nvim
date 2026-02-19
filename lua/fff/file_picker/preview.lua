local utils = require('fff.utils')
local file_picker = require('fff.file_picker')
local image = require('fff.file_picker.image')
local location_utils = require('fff.location_utils')

local M = {}

-- Additional fallback for certain ambiguous filetypes which vim.filetype.match is not handling correctly
local function get_fixed_filetype_detection(extension)
  local extension_map = {
    ts = 'typescript',
    tex = 'latex',
    md = 'markdown',
    txt = 'text',
  }

  return extension_map[extension]
end

local function detect_filetype(file_path)
  local has_plenary, plenary_filetype = pcall(require, 'plenary.filetype')
  if has_plenary then
    local detected = plenary_filetype.detect(file_path)
    if detected and detected ~= '' then return detected end
  end

  local builtin_filetype = vim.filetype.match({ filename = file_path })
  if builtin_filetype and builtin_filetype ~= '' then return builtin_filetype end

  local extension = vim.fn.fnamemodify(file_path, ':e'):lower()
  return get_fixed_filetype_detection(extension)
end

local function set_buffer_lines(bufnr, lines)
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then return end

  vim.api.nvim_buf_set_option(bufnr, 'modifiable', true)
  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, lines)
  vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
end

local function append_buffer_lines(bufnr, lines)
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then return end
  if not lines or #lines == 0 then return end

  vim.api.nvim_buf_set_option(bufnr, 'modifiable', true)
  local current_lines = vim.api.nvim_buf_line_count(bufnr)
  vim.api.nvim_buf_set_lines(bufnr, current_lines, current_lines, false, lines)
  vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
end

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

local function cleanup_file_operation()
  if M.state.file_operation then
    if M.state.file_operation.fd then pcall(vim.uv.fs_close, M.state.file_operation.fd) end
    M.state.file_operation = nil
  end
end

--- Process raw chunk data into complete lines, joining any leftover bytes
--- from the previous chunk and storing any trailing partial line for the next.
--- @param data string Raw chunk data
--- @return string[] Complete lines (may be empty if the entire chunk is a partial line)
local function split_chunk_with_remainder(data)
  if not data or data == '' then return {} end

  local fo = M.state.file_operation
  local prefix = fo and fo.remainder or ''
  local combined = prefix .. data

  local lines = vim.split(combined, '\n', { plain = true })

  if combined:sub(-1) ~= '\n' then
    -- Data doesn't end on a line boundary: last element is a partial line
    local partial = table.remove(lines) or ''
    if fo then fo.remainder = partial end
  else
    -- Data ends on a line boundary: remove the trailing empty element
    if #lines > 0 and lines[#lines] == '' then table.remove(lines) end
    if fo then fo.remainder = '' end
  end

  return lines
end

local function init_dynamic_loading_async(file_path, callback)
  cleanup_file_operation()

  M.state.loaded_lines = 0
  M.state.total_file_lines = nil
  M.state.has_more_content = true
  M.state.is_loading = false

  local generation = M.state.preview_generation

  vim.uv.fs_open(file_path, 'r', 438, function(err, fd)
    -- Stale callback: preview moved on to a different file
    if M.state.preview_generation ~= generation then
      if fd then pcall(vim.uv.fs_close, fd) end
      return
    end

    if err or not fd then
      callback(false, 'Failed to open file: ' .. (err or 'unknown error'))
      return
    end

    M.state.file_operation = {
      fd = fd,
      file_path = file_path,
      position = 0,
      remainder = '',
    }

    callback(true)
  end)
end

local function load_forward_chunk_async(target_size, callback)
  if not M.state.file_operation or not M.state.file_operation.fd then
    callback('', 'No file handle available')
    return
  end

  M.state.is_loading = true
  local chunk_size = target_size or (M.config.chunk_size or 16384)
  local generation = M.state.preview_generation

  vim.uv.fs_read(M.state.file_operation.fd, chunk_size, M.state.file_operation.position, function(err, data)
    vim.schedule(function()
      -- Stale callback: a newer preview has started, discard this result
      if M.state.preview_generation ~= generation then return end

      M.state.is_loading = false

      if err then
        callback('', 'Read error: ' .. err)
        return
      end

      if not data or #data == 0 then
        M.state.has_more_content = false
        -- Flush any remaining partial line as the final piece of data
        local final_remainder = M.state.file_operation and M.state.file_operation.remainder or ''
        cleanup_file_operation()
        if final_remainder ~= '' then
          callback(final_remainder .. '\n', nil)
        else
          callback('', nil)
        end
        return
      end

      if M.state.file_operation then M.state.file_operation.position = M.state.file_operation.position + #data end

      callback(data, nil)
    end)
  end)
end

local function load_next_chunk_async(chunk_size, callback)
  if not M.state.file_operation or not M.state.has_more_content or M.state.is_loading then
    callback('', nil)
    return
  end
  load_forward_chunk_async(chunk_size, callback)
end

-- Forward declaration for ensure_content_loaded_async (used in read_file_streaming_async callback)
local ensure_content_loaded_async

local function read_file_streaming_async(file_path, bufnr, callback)
  local generation = M.state.preview_generation

  init_dynamic_loading_async(file_path, function(success, error_msg)
    if M.state.preview_generation ~= generation then return end

    if not success then
      callback(nil, error_msg)
      return
    end

    -- Calculate initial chunk size based on location information
    local initial_chunk_size = M.config.chunk_size
    if M.state.location then
      local target_line = location_utils.get_target_line(M.state.location)
      if target_line then
        -- Estimate bytes needed: assume ~100 bytes per line average
        -- Add some buffer (50%) to account for variation in line lengths
        local estimated_bytes = target_line * 100 * 1.5
        -- Cap at reasonable maximum to avoid memory issues
        local max_initial_chunk = M.config.max_size or (10 * 1024 * 1024) -- 10MB default
        initial_chunk_size = math.min(estimated_bytes, max_initial_chunk)
        -- Ensure we don't go below the standard chunk size
        initial_chunk_size = math.max(initial_chunk_size, M.config.chunk_size)
      end
    end

    load_next_chunk_async(initial_chunk_size, function(data, err)
      if M.state.preview_generation ~= generation then return end

      if data and data ~= '' then
        local lines = split_chunk_with_remainder(data)
        M.state.loaded_lines = #lines
        M.state.content_height = #lines

        -- If we have a location and didn't load enough lines, try to load more
        local loading_more = false
        if M.state.location then
          local target_line = location_utils.get_target_line(M.state.location)
          if target_line and #lines < target_line and M.state.has_more_content then
            loading_more = true
            vim.schedule(function()
              if M.state.preview_generation == generation then ensure_content_loaded_async(target_line) end
            end)
          end
        end

        callback(lines, err, loading_more)
      else
        callback(nil, err)
      end
    end)
  end)
end

ensure_content_loaded_async = function(target_line)
  if not M.state.bufnr or not vim.api.nvim_buf_is_valid(M.state.bufnr) then return end
  if not M.state.has_more_content or M.state.is_loading then return end
  -- Guard against missing file handle: without it load_next_chunk_async returns
  -- synchronously with empty data, which triggers apply_location_highlighting
  -- -> ensure_content_loaded_async again, causing infinite recursion (stack overflow).
  if not M.state.file_operation then
    M.state.has_more_content = false
    return
  end

  local current_buffer_lines = vim.api.nvim_buf_line_count(M.state.bufnr)
  local buffer_needed = target_line + 50

  if current_buffer_lines >= buffer_needed then return end

  local generation = M.state.preview_generation

  -- Use a larger chunk to reach the target faster instead of many small 8KB reads
  local lines_needed = buffer_needed - current_buffer_lines
  local estimated_bytes = math.max(M.config.chunk_size, lines_needed * 120)

  load_next_chunk_async(estimated_bytes, function(data, err)
    -- Stale callback: preview moved on to a different file
    if M.state.preview_generation ~= generation then return end
    if not M.state.bufnr or not vim.api.nvim_buf_is_valid(M.state.bufnr) then return end

    if err then return end

    if data and data ~= '' then
      local chunk_lines = split_chunk_with_remainder(data)
      if #chunk_lines > 0 then append_buffer_lines(M.state.bufnr, chunk_lines) end

      M.state.content_height = vim.api.nvim_buf_line_count(M.state.bufnr)
      M.state.loaded_lines = M.state.content_height

      -- If we still haven't loaded enough, schedule another chunk
      if M.state.loaded_lines < buffer_needed and M.state.has_more_content then
        vim.schedule(function()
          if M.state.preview_generation == generation then ensure_content_loaded_async(target_line) end
        end)
      else
        -- Enough content loaded — re-apply location highlighting so the
        -- preview scrolls to the correct line now that it exists in the buffer
        M.apply_location_highlighting(M.state.bufnr)
      end
    else
      -- EOF with no additional data — mark loading as finished to prevent
      -- apply_location_highlighting -> ensure_content_loaded_async recursion,
      -- then apply highlighting with whatever content we have.
      M.state.has_more_content = false
      M.apply_location_highlighting(M.state.bufnr)
    end
  end)
end

local function link_buffer_content(source_bufnr, target_bufnr)
  local lines = vim.api.nvim_buf_get_lines(source_bufnr, 0, -1, false)
  set_buffer_lines(target_bufnr, lines)

  local source_ft = vim.api.nvim_buf_get_option(source_bufnr, 'filetype')
  if source_ft ~= '' then vim.api.nvim_buf_set_option(target_bufnr, 'filetype', source_ft) end

  M.state.has_more_content = false
  M.state.total_file_lines = #lines
  M.state.loaded_lines = #lines
  M.state.content_height = #lines

  return true
end

M.config = nil

M.state = {
  bufnr = nil,
  winid = nil,
  current_file = nil,
  scroll_offset = 0,
  content_height = 0,
  loaded_lines = 0,
  total_file_lines = nil,
  loading_chunk_size = 1000,
  is_loading = false,
  has_more_content = true,
  file_handle = nil,
  file_operation = nil, -- Ongoing file operation: {fd?: any, file_path?: string, position?: number}
  location = nil, -- Current location data for highlighting
  location_namespace = nil, -- Namespace for location highlighting
  preview_generation = 0, -- Monotonically increasing token to detect stale async callbacks
}

--- Setup preview configuration
--- @param config table Configuration options
function M.setup(config)
  M.config = config or {}
  -- Create namespace for location highlighting
  if not M.state.location_namespace then
    M.state.location_namespace = vim.api.nvim_create_namespace('fff_preview_location')
  end
end

--- Check if file is too big for initial preview (inspired by snacks.nvim)
--- @param file_path string Path to the file
--- @param bufnr number|nil Buffer number to check (unused with dynamic loading)
--- @return boolean True if file is too big for initial preview
function M.is_big_file(file_path, bufnr)
  -- Only check file size for early detection - no line limits with dynamic loading
  local stat = vim.uv.fs_stat(file_path)
  if stat and stat.size > M.config.max_size then return true end

  return false
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
  info.filetype = detect_filetype(file_path) or 'text'
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
      string.format(
        'Score Modifiers: frec_boost=%d, dist_penalty=%d, current_penalty=%d',
        score.frecency_boost,
        score.distance_penalty,
        score.current_file_penalty or 0
      )
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

--- Create file info content for grep mode items.
--- Shows grep-specific metadata: match location, frecency, file info.
---@param item table Grep match item with file + match metadata
---@param info table File system information from get_file_info
---@return table Lines for the file info content
function M.create_grep_file_info_content(item, info)
  local lines = {}

  -- Match location info
  local match_count = item.match_ranges and #item.match_ranges or 0
  table.insert(
    lines,
    string.format('Match: line %d, col %d │ Ranges: %d', item.line_number or 0, (item.col or 0) + 1, match_count)
  )
  table.insert(
    lines,
    string.format('Byte Offset: %-12d │ Size: %s', item.byte_offset or 0, info.size_formatted or 'N/A')
  )
  table.insert(lines, string.format('Type: %-8s │ Git: %s', info.filetype or 'text', item.git_status or 'clean'))

  -- Fuzzy match score (only available in fuzzy grep mode)
  if item.fuzzy_score then table.insert(lines, string.format('Fuzzy Score: %d', item.fuzzy_score)) end

  -- Frecency info
  local total = item.total_frecency_score or 0
  local acc = item.access_frecency_score or 0
  local mod = item.modification_frecency_score or 0
  table.insert(lines, string.format('Frecency: total=%d, access=%d, modification=%d', total, acc, mod))

  -- Ordering explanation
  table.insert(lines, 'Order: files sorted by frecency desc, matches by line asc')
  table.insert(lines, '')

  -- Time information section
  table.insert(lines, 'TIMINGS')
  table.insert(lines, string.rep('─', 50))
  table.insert(lines, string.format('Modified: %s', info.modified_formatted or 'N/A'))
  table.insert(lines, string.format('Last Access: %s', info.accessed_formatted or 'N/A'))

  return lines
end

--- Preview a regular file
--- @param file_path string Path to the file
--- @param bufnr number Buffer number for preview
--- @return boolean Success status
function M.preview_file(file_path, bufnr)
  -- Early size detection to prevent memory issues
  if M.is_big_file(file_path, bufnr) then
    local info = M.get_file_info(file_path)
    local lines = {
      'File too large for preview',
      string.format(
        'Size: %s (max: %s)',
        info and info.size_formatted or 'Unknown',
        string.format('%.1fMB', M.config.max_size / 1024 / 1024)
      ),
      '',
      'Use a text editor to view this file.',
    }
    set_buffer_lines(bufnr, lines)
    return true
  end

  local info = M.get_file_info(file_path)
  if not info then return false end

  -- if the buffer is already opened for this file we reuse the buffer directly
  local existing_bufnr = find_existing_buffer(file_path)

  if existing_bufnr then
    local success = link_buffer_content(existing_bufnr, bufnr)
    if success then
      local file_config = M.get_file_config(file_path)

      vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
      vim.api.nvim_buf_set_option(bufnr, 'readonly', true)
      vim.api.nvim_buf_set_option(bufnr, 'buftype', 'nofile')
      vim.api.nvim_buf_set_option(bufnr, 'wrap', file_config.wrap_lines or M.config.wrap_lines)

      M.state.scroll_offset = 0

      -- Apply location highlighting if available (delayed to ensure buffer is ready)
      local gen = M.state.preview_generation
      vim.schedule(function()
        if M.state.preview_generation == gen then M.apply_location_highlighting(bufnr) end
      end)

      return true
    end
  end

  M.state.current_file = file_path
  M.state.bufnr = bufnr
  local generation = M.state.preview_generation

  read_file_streaming_async(file_path, bufnr, function(content, err, loading_more)
    if M.state.preview_generation ~= generation then
      -- Preview moved on to a different file, discard
      cleanup_file_operation()
      return
    end

    if err or not content then
      if M.state.current_file == file_path then
        set_buffer_lines(bufnr, { 'Failed to load file: ' .. (err or 'unknown error') })
      end
      return
    end

    if M.state.current_file == file_path then
      -- Guard against buffer being destroyed while async read was in-flight
      if not vim.api.nvim_buf_is_valid(bufnr) then return end

      M.clear_preview_visual_state(bufnr)
      set_buffer_lines(bufnr, content)

      local file_config = M.get_file_config(file_path)
      vim.api.nvim_buf_set_option(bufnr, 'filetype', info.filetype)
      vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
      vim.api.nvim_buf_set_option(bufnr, 'readonly', true)
      vim.api.nvim_buf_set_option(bufnr, 'buftype', 'nofile')
      vim.api.nvim_buf_set_option(bufnr, 'wrap', file_config.wrap_lines or M.config.wrap_lines)

      M.state.content_height = #content
      M.state.scroll_offset = 0

      -- Apply location highlighting if available (delayed to ensure buffer is ready).
      -- Skip when more content is being loaded asynchronously to reach the target line —
      -- ensure_content_loaded_async will re-apply highlighting once the target is in the buffer.
      if not loading_more then
        vim.schedule(function()
          if M.state.preview_generation == generation then M.apply_location_highlighting(bufnr) end
        end)
      end
    end
  end)

  return true
end

--- Preview a binary file with async file type detection
--- @param file_path string Path to the file
--- @param bufnr number Buffer number for preview
--- @return boolean Success status
function M.preview_binary_file(file_path, bufnr)
  local info = M.get_file_info(file_path)
  local lines = {}

  set_buffer_lines(bufnr, lines)
  vim.api.nvim_buf_set_option(bufnr, 'filetype', 'text')
  vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
  vim.api.nvim_buf_set_option(bufnr, 'readonly', true)

  if vim.fn.executable('file') == 1 then
    local cmd = { 'file', '-b', file_path }
    vim.system(cmd, { text = true }, function(result)
      vim.schedule(function()
        if not vim.api.nvim_buf_is_valid(bufnr) then return end

        if result.code == 0 and result.stdout then
          local file_type = result.stdout:gsub('\n', '')
          table.insert(lines, 'Binary file: ' .. file_type)
          if info and info.size_formatted then table.insert(lines, 'Size: ' .. info.size_formatted) end

          if vim.fn.executable('xxd') == 1 then
            table.insert(lines, '')
            set_buffer_lines(bufnr, lines)

            local hex_cmd = { 'xxd', '-l', '8192', file_path }
            vim.system(hex_cmd, { text = true }, function(hex_result)
              vim.schedule(function()
                if not vim.api.nvim_buf_is_valid(bufnr) then return end

                if hex_result.code == 0 and hex_result.stdout then
                  local hex_lines = vim.split(hex_result.stdout, '\n')
                  for _, line in ipairs(hex_lines) do
                    if line:match('%S') then table.insert(lines, line) end
                  end
                else
                  table.insert(lines, 'Use a hex editor or appropriate application to view this file.')
                end
                set_buffer_lines(bufnr, lines)
              end)
            end)
          else
            table.insert(lines, 'Use a hex editor or appropriate application to view this file.')
            set_buffer_lines(bufnr, lines)
          end
        end
      end)
    end)
  end

  return true
end

--- Get file-specific configuration
--- @param file_path string Path to the file
--- @return table Configuration for the file
function M.get_file_config(file_path)
  if not M.config or not M.config.filetypes then return {} end

  local filetype = detect_filetype(file_path) or 'text'
  return M.config.filetypes[filetype] or {}
end

--- @param file_path string Path to the file or directory
--- @param bufnr number Buffer number for preview
--- @param location table|nil Optional location data for highlighting
--- @param is_binary boolean|nil Whether the file is binary (from Rust indexer)
--- @return boolean if the preview was successful
function M.preview(file_path, bufnr, location, is_binary)
  if not file_path or file_path == '' then return false end

  -- Bump generation to invalidate any in-flight async callbacks from previous previews
  M.state.preview_generation = M.state.preview_generation + 1

  if M.state.file_handle then
    M.state.file_handle:close()
    M.state.file_handle = nil
  end

  M.state.loaded_lines = 0
  M.state.total_file_lines = nil
  M.state.has_more_content = true
  M.state.is_loading = false

  M.state.current_file = file_path
  M.state.bufnr = bufnr
  M.state.location = location

  if image.is_image(file_path) then
    M.clear_buffer(bufnr)

    if not M.state.winid or not vim.api.nvim_win_is_valid(M.state.winid) then return false end

    local win_width = vim.api.nvim_win_get_width(M.state.winid) - 2
    local win_height = vim.api.nvim_win_get_height(M.state.winid) - 2

    return image.display_image(file_path, bufnr, win_width, win_height)
  elseif is_binary then
    return M.preview_binary_file(file_path, bufnr)
  else
    return M.preview_file(file_path, bufnr)
  end
end

function M.scroll(lines)
  if not M.state.bufnr or not vim.api.nvim_buf_is_valid(M.state.bufnr) then return end
  if not M.state.winid or not vim.api.nvim_win_is_valid(M.state.winid) then return end

  local win_height = vim.api.nvim_win_get_height(M.state.winid)
  local current_buffer_lines = vim.api.nvim_buf_line_count(M.state.bufnr)

  local current_offset = M.state.scroll_offset or 0
  local new_offset = current_offset + lines

  -- If scrolling down and approaching end of loaded content, try to load more
  if lines > 0 and not M.state.is_loading then
    local target_line = new_offset + win_height
    local buffer_needed = target_line + 20 -- Load a bit ahead

    if current_buffer_lines < buffer_needed and M.state.has_more_content then
      -- Load more content asynchronously but don't wait for it
      ensure_content_loaded_async(target_line)
    end
  end

  -- Use actual buffer line count for scroll calculations
  local content_height = current_buffer_lines
  local half_screen = math.floor(win_height / 2)
  local max_scroll = math.max(0, content_height + half_screen - win_height)

  new_offset = math.max(0, math.min(max_scroll, new_offset))
  if new_offset ~= current_offset then
    M.state.scroll_offset = new_offset
    M.state.content_height = content_height

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

--- Update file info buffer
--- @param file table File information from search results (or grep match item)
--- @param bufnr number Buffer number for file info
--- @param file_index number|nil Index of the file in search results (for score lookup, file mode only)
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

  -- Detect grep mode items by the presence of line_number (grep-specific field)
  local file_info_lines
  if file.line_number ~= nil then
    file_info_lines = M.create_grep_file_info_content(file, info)
  else
    file_info_lines = M.create_file_info_content(file, info, file_index)
  end
  set_buffer_lines(bufnr, file_info_lines)

  vim.api.nvim_buf_set_option(bufnr, 'modifiable', false)
  vim.api.nvim_buf_set_option(bufnr, 'readonly', true)
  vim.api.nvim_buf_set_option(bufnr, 'buftype', 'nofile')
  vim.api.nvim_buf_set_option(bufnr, 'wrap', false)

  return true
end

function M.clear_preview_visual_state(bufnr)
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then return end

  -- Only clear visual state, don't affect buffer functionality
  -- Clear namespaces and extmarks for this buffer only
  vim.api.nvim_buf_clear_namespace(bufnr, -1, 0, -1)

  -- Clear location highlights
  if M.state.location_namespace then location_utils.clear_location_highlights(bufnr, M.state.location_namespace) end

  local wins = vim.fn.win_findbuf(bufnr)

  for _, win in ipairs(wins) do
    if vim.api.nvim_win_is_valid(win) then
      -- Reset folds
      pcall(vim.api.nvim_win_call, win, function()
        if vim.fn.has('folding') == 1 then
          vim.cmd('normal! zE') -- eliminate all folds
          vim.opt_local.foldenable = false -- disable folding
        end
      end)
    end
  end

  image.clear_buffer_images(bufnr)
end

function M.clear_buffer(bufnr)
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then return end

  cleanup_file_operation()
  M.clear_preview_visual_state(bufnr)

  pcall(vim.treesitter.stop, bufnr)

  vim.api.nvim_buf_set_option(bufnr, 'modifiable', true)
  vim.api.nvim_buf_set_option(bufnr, 'filetype', '')
  vim.api.nvim_buf_set_option(bufnr, 'syntax', '')
  vim.api.nvim_buf_set_option(bufnr, 'buftype', 'nofile')

  set_buffer_lines(bufnr, {})
end

function M.clear()
  -- Bump generation to invalidate any in-flight async callbacks
  M.state.preview_generation = M.state.preview_generation + 1

  cleanup_file_operation()

  M.state.loaded_lines = 0
  M.state.total_file_lines = nil
  M.state.has_more_content = true
  M.state.is_loading = false

  if M.state.bufnr and vim.api.nvim_buf_is_valid(M.state.bufnr) then M.clear_buffer(M.state.bufnr) end

  M.state.current_file = nil
  M.state.scroll_offset = 0
  M.state.content_height = 0
  M.state.location = nil
end

--- Apply location highlighting to the preview buffer
--- @param bufnr number Buffer number
function M.apply_location_highlighting(bufnr)
  -- Ensure namespace is created
  if not M.state.location_namespace then
    M.state.location_namespace = vim.api.nvim_create_namespace('fff_preview_location')
  end

  -- Always clear previous location highlights first
  if vim.api.nvim_buf_is_valid(bufnr) then
    location_utils.clear_location_highlights(bufnr, M.state.location_namespace)
  end

  if not M.state.location then return end

  -- Apply highlighting
  location_utils.highlight_location(bufnr, M.state.location, M.state.location_namespace)

  if M.state.winid and vim.api.nvim_win_is_valid(M.state.winid) then
    local target_line = location_utils.get_target_line(M.state.location)
    if target_line then
      local buffer_lines = vim.api.nvim_buf_line_count(bufnr)
      if target_line > buffer_lines and M.state.has_more_content then
        -- Target line is beyond loaded content — load more first.
        -- ensure_content_loaded_async will re-apply highlighting when done.
        ensure_content_loaded_async(target_line)
        return
      end
      M.scroll_to_line(target_line)
    end
  end
end

--- Scroll preview to a specific line
--- @param line number Target line number (1-indexed)
function M.scroll_to_line(line)
  if not M.state.winid or not vim.api.nvim_win_is_valid(M.state.winid) then return end
  if not M.state.bufnr or not vim.api.nvim_buf_is_valid(M.state.bufnr) then return end

  local win_height = vim.api.nvim_win_get_height(M.state.winid)
  local buffer_lines = vim.api.nvim_buf_line_count(M.state.bufnr)
  local target_line = math.max(1, math.min(line, buffer_lines))

  local half_screen = math.floor(win_height / 2)
  local new_offset = math.max(0, target_line - half_screen)

  M.state.scroll_offset = new_offset
  pcall(vim.api.nvim_win_call, M.state.winid, function()
    vim.api.nvim_win_set_cursor(M.state.winid, { target_line, 0 })
    vim.cmd('normal! zt')
  end)
end

return M
