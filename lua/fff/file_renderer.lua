--- File Renderer
--- Simple renderer for file items with 2 functions: render_line and apply_highlights
local M = {}

--- Render Context passed to renderer functions
--- @class RenderContext
--- @field config table User configuration from conf.get()
--- @field items table[] Array of file items being rendered
--- @field cursor number Current cursor position (1-based index into items)
--- @field win_height number Window height in lines
--- @field win_width number Window width in columns
--- @field max_path_width number Maximum width for file paths
--- @field debug_enabled boolean Whether debug mode is enabled (shows frecency scores)
--- @field prompt_position string Prompt position: 'top' or 'bottom'
--- @field has_combo boolean Whether combo boost is active
--- @field combo_header_line string Formatted combo header line (if has_combo)
--- @field combo_header_text_len number Length of combo header text (if has_combo)
--- @field combo_item_index number Index of item with combo (usually 1)
--- @field display_start number Start index for displayed items
--- @field display_end number End index for displayed items
--- @field iter_start number Iteration start (may differ from display_start for bottom prompt)
--- @field iter_end number Iteration end (may differ from display_end for bottom prompt)
--- @field iter_step number Iteration step (1 for top prompt, -1 for bottom prompt)
--- @field format_file_display fun(item: table, max_width: number): string, string Helper function to format filename and dir path
--- @field selected_files table<string, boolean> Map of selected file paths
--- @field query string Current search query
--- @field renderer table|nil Custom renderer (if provided via opts)

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
--- @field git_status number|nil Git status enum (if file is in git repo)

--- Renderer Interface:
--- @field render_line fun(item: FileItem, ctx: RenderContext, item_idx: number): string[] Returns array of line strings
--- @field apply_highlights fun(item: FileItem, ctx: RenderContext, item_idx: number, buf: number, ns_id: number, line_idx: number, line_content: string): nil Applies highlights to the rendered line

--- Render a file item line
--- @param item FileItem File item from Rust
--- @param ctx RenderContext Render context with all state
--- @param item_idx number Item index (1-based)
--- @return string[] Array of line strings (1 or 2 lines if combo)
function M.render_line(item, ctx, item_idx)
  local icons = require('fff.file_picker.icons')
  local lines = {}

  -- Check if this should have combo header (first item with combo boost)
  local has_combo = item_idx == 1 and ctx.has_combo and ctx.combo_header_line

  if has_combo then table.insert(lines, ctx.combo_header_line) end

  -- Get icon
  local icon, icon_hl_group = icons.get_icon(item.name, item.extension, false)

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
        indicator = '⭐'
      elseif total >= 3 then
        indicator = '✨'
      elseif total >= 1 then
        indicator = '•'
      end
      frecency = string.format(' %s%d', indicator, total)
    end
  end

  -- Format filename and path
  local icon_width = icon and (vim.fn.strdisplaywidth(icon) + 1) or 0
  local available_width = math.max(ctx.max_path_width - icon_width - #frecency, 40)
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
--- @param ctx RenderContext Render context with all state
--- @param item_idx number Item index (1-based)
--- @param buf number Buffer handle
--- @param ns_id number Namespace ID
--- @param line_idx number 1-based line index in buffer
--- @param line_content string The actual line content
function M.apply_highlights(item, ctx, item_idx, buf, ns_id, line_idx, line_content)
  local icons = require('fff.file_picker.icons')
  local git_utils = require('fff.git_utils')
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
      hl_group = ctx.config.hl.active_file,
      hl_eol = true,
      priority = 100,
    })
  end

  -- 2. Icon
  if icon and icon_hl_group and vim.fn.strdisplaywidth(icon) > 0 then
    local icon_hl = is_current_file and 'Comment' or icon_hl_group
    vim.api.nvim_buf_add_highlight(buf, ns_id, icon_hl, line_idx - 1, 0, vim.fn.strdisplaywidth(icon))
  end

  -- 3. Git text color (filename)
  if ctx.config.git and ctx.config.git.status_text_color and icon and #filename > 0 then
    local git_text_hl = item.git_status and git_utils.get_text_highlight(item.git_status) or nil
    if git_text_hl and git_text_hl ~= '' and not is_current_file then
      local filename_start = #icon + 1
      vim.api.nvim_buf_add_highlight(buf, ns_id, git_text_hl, line_idx - 1, filename_start, filename_start + #filename)
    end
  end

  -- 4. Frecency indicator
  if ctx.debug_enabled then
    local start_pos, end_pos = line_content:find('[⭐🔥✨•]%d+')
    if start_pos then
      vim.api.nvim_buf_add_highlight(buf, ns_id, ctx.config.hl.frecency, line_idx - 1, start_pos - 1, end_pos)
    end
  end

  -- 5. Directory path (dimmed)
  if #filename > 0 and #dir_path > 0 then
    local prefix_len = #filename + 1 -- filename bytes + space
    if icon then
      prefix_len = prefix_len + #icon + 1 -- if icon add icon bytes + space
    end
    vim.api.nvim_buf_add_highlight(
      buf,
      ns_id,
      ctx.config.hl.directory_path or 'Comment',
      line_idx - 1,
      prefix_len,
      prefix_len + #dir_path
    )
  end

  -- 6. Current file
  if is_current_file then
    if not is_cursor then vim.api.nvim_buf_add_highlight(buf, ns_id, 'Comment', line_idx - 1, 0, -1) end
    local virt_text_hl = is_cursor and ctx.config.hl.active_file or 'Comment'
    vim.api.nvim_buf_set_extmark(buf, ns_id, line_idx - 1, 0, {
      virt_text = { { ' (current)', virt_text_hl } },
      virt_text_pos = 'right_align',
    })
  end

  -- 7. Git sign
  if item.git_status and git_utils.should_show_border(item.git_status) then
    local border_char = git_utils.get_border_char(item.git_status)
    local border_hl

    if is_cursor then
      local base_hl = git_utils.get_border_highlight(item.git_status)
      if base_hl and base_hl ~= '' then
        local border_fg = vim.fn.synIDattr(vim.fn.synIDtrans(vim.fn.hlID(base_hl)), 'fg')
        local cursor_bg = vim.fn.synIDattr(vim.fn.synIDtrans(vim.fn.hlID(ctx.config.hl.active_file)), 'bg')
        local temp_hl_name = 'FFFGitBorderSelected_' .. item_idx
        if border_fg ~= '' and cursor_bg ~= '' then
          vim.api.nvim_set_hl(0, temp_hl_name, { fg = border_fg, bg = cursor_bg })
          border_hl = temp_hl_name
        else
          border_hl = git_utils.get_border_highlight_selected(item.git_status)
        end
      else
        border_hl = ctx.config.hl.active_file
      end
    else
      border_hl = git_utils.get_border_highlight(item.git_status)
    end

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
      sign_hl_group = ctx.config.hl.active_file,
      priority = 1000,
    })
  end

  -- 8. Selection
  if ctx.selected_files and ctx.selected_files[item.path] then
    local selection_hl = is_cursor and ctx.config.hl.selected_active or ctx.config.hl.selected
    vim.api.nvim_buf_set_extmark(buf, ns_id, line_idx - 1, 0, {
      sign_text = '▊',
      sign_hl_group = selection_hl,
      priority = 1001,
    })
  end

  -- 9. Query match
  if ctx.query and ctx.query ~= '' then
    local match_start, match_end = string.find(line_content, ctx.query, 1)
    if match_start and match_end then
      vim.api.nvim_buf_add_highlight(
        buf,
        ns_id,
        ctx.config.hl.matched or 'IncSearch',
        line_idx - 1,
        match_start - 1,
        match_end
      )
    end
  end
end

return M
