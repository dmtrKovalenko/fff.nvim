--- File Renderer
--- Simple renderer for file items with 2 functions: render_line and apply_highlights
local M = {}

--- File Item structure from Rust
--- @class FileItem
--- @field path string Absolute file path
--- @field relative_path string Relative file path from base directory
--- @field name string File name
--- @field extension string File extension
--- @field size number File size in bytes
--- @field modified number Last modified timestamp
--- @field total_frecency_score number Total frecency score
--- @field access_frecency_score number Access-based frecency score
--- @field modification_frecency_score number Modification-based frecency score
--- @field git_status string|nil Git status string (e.g. 'modified', 'untracked') if file is in git repo
--- @field match_ranges number[][]|nil Byte ranges for fuzzy query matches

--- Render a file item line
--- @param item FileItem File item from Rust
--- @param ctx ListRenderContext Render context with all state
--- @param item_idx number|nil 1-based item index in ctx.items
--- @return string[] Array of line strings (always exactly 1)
function M.render_line(item, ctx, item_idx) -- luacheck: ignore item_idx
  local icons = require('fff.file_picker.icons')
  local lines = {}

  local icon, _ = icons.get_icon(item.name, item.extension, false)

  -- Build frecency indicator (debug mode only)
  local frecency = ''
  if ctx.debug_enabled then
    local total = item.total_frecency_score or 0
    local access = item.access_frecency_score or 0
    local mod = item.modification_frecency_score or 0

    if total > 0 then
      local indicator = ''
      if mod >= 6 then
        indicator = '🔥'
      elseif access >= 4 then
        indicator = '⭐️'
      elseif total >= 3 then
        indicator = '✨'
      elseif total >= 1 then
        indicator = '•'
      end
      frecency = string.format(' %s%d', indicator, total)
    end
  end

  -- Format filename and path
  -- Don't reserve space for frecency - path takes priority
  local icon_width = icon and (vim.fn.strdisplaywidth(icon) + 1) or 0
  local available_width = math.max(ctx.max_path_width - icon_width, 40)
  local filename, dir_path = ctx.format_file_display(item, available_width)

  -- Build line
  local line = icon and string.format('%s %s %s%s', icon, filename, dir_path, frecency)
    or string.format('%s %s%s', filename, dir_path, frecency)

  local padding = math.max(0, ctx.win_width - vim.fn.strdisplaywidth(line) + 5)
  table.insert(lines, line .. string.rep(' ', padding))

  return lines
end

--- Apply highlights to a rendered line
--- @param item FileItem File item from Rust
--- @param ctx ListRenderContext Render context with all state
--- @param item_idx number Item index (1-based)
--- @param buf number Buffer handle
--- @param ns_id number Namespace ID
--- @param line_idx number 1-based line index in buffer
--- @param line_content string The actual line content
function M.apply_highlights(item, ctx, item_idx, buf, ns_id, line_idx, line_content)
  local icons = require('fff.file_picker.icons')
  local highlights = require('fff.highlights')
  local file_picker = require('fff.file_picker')

  local is_cursor = (ctx.cursor == item_idx)
  local score = file_picker.get_file_score(item_idx)
  local is_current_file = score and score.current_file_penalty and score.current_file_penalty < 0

  -- Get icon and paths
  local icon, icon_hl_group = icons.get_icon(item.name, item.extension, false)
  local icon_width = icon and (vim.fn.strdisplaywidth(icon) + 1) or 0
  local available_width = math.max(ctx.max_path_width - icon_width, 40)
  local filename, dir_path = ctx.format_file_display(item, available_width)

  -- 1. Cursor highlight
  if is_cursor then
    vim.api.nvim_buf_set_extmark(buf, ns_id, line_idx - 1, 0, {
      end_col = 0,
      end_row = line_idx,
      hl_group = ctx.config.hl.cursor,
      hl_eol = true,
      priority = 100,
    })
  end

  -- 2. Icon
  if icon and icon_hl_group and vim.fn.strdisplaywidth(icon) > 0 then
    local icon_hl = is_current_file and 'Comment' or icon_hl_group
    vim.api.nvim_buf_set_extmark(
      buf,
      ns_id,
      line_idx - 1,
      0,
      { end_col = vim.fn.strdisplaywidth(icon), hl_group = icon_hl }
    )
  end

  -- 3. Git text color (filename)
  if ctx.config.git and ctx.config.git.status_text_color and icon and #filename > 0 then
    local git_text_hl = item.git_status and highlights.get_git_text_highlight(item.git_status) or nil
    if git_text_hl and git_text_hl ~= '' and not is_current_file then
      local filename_start = #icon + 1
      vim.api.nvim_buf_set_extmark(
        buf,
        ns_id,
        line_idx - 1,
        filename_start,
        { end_col = filename_start + #filename, hl_group = git_text_hl }
      )
    end
  end

  -- 4. Frecency indicator
  if ctx.debug_enabled then
    local start_pos, end_pos = line_content:find('[⭐️🔥✨•]%d+')
    if start_pos and end_pos then
      vim.api.nvim_buf_set_extmark(
        buf,
        ns_id,
        line_idx - 1,
        start_pos - 1,
        { end_col = end_pos, hl_group = ctx.config.hl.frecency }
      )
    end
  end

  -- 5. Directory path (dimmed)
  if #filename > 0 and #dir_path > 0 then
    local prefix_len = #filename + 1 -- filename bytes + space
    if icon then
      prefix_len = prefix_len + #icon + 1 -- if icon add icon bytes + space
    end
    vim.api.nvim_buf_set_extmark(
      buf,
      ns_id,
      line_idx - 1,
      prefix_len,
      { end_col = prefix_len + #dir_path, hl_group = ctx.config.hl.directory_path }
    )
  end

  -- 6. Current file
  if is_current_file then
    local hl
    if is_cursor then
      hl = ctx.config.hl.cursor
    else
      hl = 'Comment'
    end

    vim.api.nvim_buf_set_extmark(buf, ns_id, line_idx - 1, 0, {
      virt_text = { { ' ' .. ctx.config.file_picker.current_file_label, hl } },
      virt_text_pos = 'right_align',
    })
  end

  -- 7. Git sign
  if item.git_status and highlights.should_show_git_border(item.git_status) then
    local border_char = highlights.get_git_border_char(item.git_status)
    local border_hl = highlights.get_git_sign_highlight(item.git_status, is_cursor, ctx.config.hl.cursor)

    if border_hl and border_hl ~= '' then
      vim.api.nvim_buf_set_extmark(buf, ns_id, line_idx - 1, 0, {
        sign_text = border_char,
        sign_hl_group = border_hl,
        priority = 1000,
      })
    end
  elseif is_cursor then
    vim.api.nvim_buf_set_extmark(buf, ns_id, line_idx - 1, 0, {
      sign_text = ' ',
      sign_hl_group = ctx.config.hl.cursor,
      priority = 1000,
    })
  end

  -- 8. Selection
  if ctx.selected_files and ctx.selected_files[item.relative_path] then
    local selection_hl = is_cursor and ctx.config.hl.selected_active or ctx.config.hl.selected
    vim.api.nvim_buf_set_extmark(buf, ns_id, line_idx - 1, 0, {
      sign_text = '▊',
      sign_hl_group = selection_hl,
      priority = 1001,
    })
  end

  -- 9. Query matches
  if ctx.query and ctx.query ~= '' then
    local matched_hl = ctx.config.hl.matched or 'IncSearch'
    local fuzzy_highlighting = ctx.config.file_picker and ctx.config.file_picker.fuzzy_query_highlighting

    if not fuzzy_highlighting then
      local match_start, match_end = string.find(line_content, ctx.query, 1, true)
      if match_start and match_end then
        vim.api.nvim_buf_set_extmark(buf, ns_id, line_idx - 1, match_start - 1, {
          end_col = match_end,
          hl_group = matched_hl,
        })
      end
      return
    end

    local ranges = item.match_ranges
    if not ranges or #ranges == 0 then return end

    local rel_path = item.relative_path or ''
    if type(rel_path) ~= 'string' then rel_path = tostring(rel_path) end

    local original_dir_path = ''
    local parent_dir = vim.fn.fnamemodify(rel_path, ':h')
    if parent_dir ~= '.' and parent_dir ~= '' then original_dir_path = parent_dir end

    local filename_rel_start = math.max(0, #rel_path - #filename)
    local filename_rel_end = filename_rel_start + #filename
    local filename_line_start = icon and (#icon + 1) or 0
    local dir_line_start = filename_line_start + #filename + 1
    local segments = { { filename_rel_start, filename_rel_end, filename_line_start } }

    if original_dir_path ~= '' and dir_path == original_dir_path then
      segments[#segments + 1] = { 0, #original_dir_path, dir_line_start }
    end

    local function apply_segment(raw_start, raw_end, segment)
      local source_start, source_end, target_start = segment[1], segment[2], segment[3]
      local start_col = math.max(raw_start, source_start)
      local end_col = math.min(raw_end, source_end)
      if end_col <= start_col then return end

      local hl_start = target_start + (start_col - source_start)
      local hl_end = target_start + (end_col - source_start)
      if hl_start < #line_content and hl_end <= #line_content then
        vim.api.nvim_buf_set_extmark(buf, ns_id, line_idx - 1, hl_start, {
          end_col = hl_end,
          hl_group = matched_hl,
          priority = 200,
        })
      end
    end

    for _, range in ipairs(ranges) do
      local raw_start = range[1] or 0
      local raw_end = range[2] or 0
      if raw_end > raw_start then
        for _, segment in ipairs(segments) do
          apply_segment(raw_start, raw_end, segment)
        end
      end
    end
  end
end

return M
