--- Grep Renderer
--- Custom renderer for live grep results with file grouping.
--- Consecutive matches from the same file are grouped under a file header line.
--- The header reuses the same rendering as the file picker list (file_renderer)
--- for visual consistency — same icon, filename, directory path, git highlights.
local M = {}

local file_renderer = require('fff.file_renderer')

--- Build the file group header line using the same layout as file_renderer.
--- Delegates to file_renderer.render_line (with combo disabled).
---@param item table Grep match item (used for file metadata)
---@param ctx table Render context
---@return string The header line string
local function build_group_header(item, ctx)
  -- file_renderer.render_line checks (item_idx == 1 and ctx.has_combo) for combo header.
  -- We pass item_idx=0 and disable has_combo to suppress combo logic entirely.
  local saved_has_combo = ctx.has_combo
  ctx.has_combo = false
  local lines = file_renderer.render_line(item, ctx, 0)
  ctx.has_combo = saved_has_combo
  return lines[1]
end

--- Apply highlights for a file group header line using file_renderer.
--- Delegates to file_renderer.apply_highlights so all highlight groups
--- (icon, filename, git text color, directory path, git sign) match exactly.
---@param item table Grep match item
---@param ctx table Render context
---@param buf number Buffer handle
---@param ns_id number Namespace id
---@param row number 0-based row in buffer (header line)
local function apply_group_header_highlights(item, ctx, buf, ns_id, row)
  local line_content = vim.api.nvim_buf_get_lines(buf, row, row + 1, false)[1] or ''
  -- file_renderer.apply_highlights uses 1-based line_idx and checks (cursor == item_idx).
  -- Pass item_idx=0 so the header is never treated as the cursor item.
  local saved_cursor = ctx.cursor
  ctx.cursor = -1
  file_renderer.apply_highlights(item, ctx, 0, buf, ns_id, row + 1, line_content)
  ctx.cursor = saved_cursor
end

--- Render a grep match line (grouped: no filename, just location + content).
--- Format: " :line:col  matched line content"
---@param item table Grep match item
---@param ctx table Render context
---@return string The match line string
local function render_match_line(item, ctx)
  local location = string.format(':%d:%d', item.line_number or 0, (item.col or 0) + 1)
  local separator = '  '
  local raw_content = item.line_content or ''
  local leading_ws = #raw_content - #raw_content:match('^%s*(.*)')
  local content = vim.trim(raw_content)

  -- Indent + location + separator + content
  local indent = ' '
  -- Prefix is always ASCII so byte length == display width
  local prefix_display_w = #indent + #location + #separator
  local available = ctx.win_width - prefix_display_w - 2
  local was_truncated = false
  local content_display_w = vim.fn.strdisplaywidth(content)
  if content_display_w > available and available > 3 then
    -- UTF-8 aware truncation: binary search for the character count that
    -- fits within the available display width (handles multi-byte and wide chars)
    local nchars = vim.fn.strchars(content)
    local lo, hi = 0, nchars
    while lo < hi do
      local mid = math.floor((lo + hi + 1) / 2)
      if vim.fn.strdisplaywidth(vim.fn.strcharpart(content, 0, mid)) <= available - 1 then
        lo = mid
      else
        hi = mid - 1
      end
    end
    content = vim.fn.strcharpart(content, 0, lo) .. '…'
    was_truncated = true
  end

  local line = indent .. location .. separator .. content
  local padding = math.max(0, ctx.win_width - vim.fn.strdisplaywidth(line) + 5)

  -- Store transient data on item for highlight pass
  item._leading_ws = leading_ws
  item._was_truncated = was_truncated
  item._match_indent = #indent
  item._content_offset = prefix_display_w -- byte offset where content starts in the line
  item._trimmed_content = content -- trimmed content string for treesitter parsing

  return line .. string.rep(' ', padding)
end

--- Apply highlights for a grouped match line.
---@param item table Grep match item
---@param ctx table Render context
---@param item_idx number 1-based item index
---@param buf number Buffer handle
---@param ns_id number Namespace id
---@param row number 0-based row in buffer
---@param line_content string The rendered line text
local function apply_match_highlights(item, ctx, item_idx, buf, ns_id, row, line_content)
  local config = ctx.config
  local is_cursor = item_idx == ctx.cursor
  local indent = item._match_indent or 1

  -- 1. Cursor line highlight — use hl_group + hl_eol instead of line_hl_group
  -- so that higher-priority inline extmarks (IncSearch match ranges at 200)
  -- cleanly override both fg and bg on the cursor line.
  if is_cursor then
    vim.api.nvim_buf_set_extmark(buf, ns_id, row, 0, {
      end_col = 0,
      end_row = row + 1,
      hl_group = config.hl.cursor,
      hl_eol = true,
      priority = 100,
    })
  end

  -- 2. Location (:line:col) dimmed — use extmark with priority so it layers with cursor
  local location_str = string.format(':%d:%d', item.line_number or 0, (item.col or 0) + 1)
  local loc_start = indent
  local loc_end = loc_start + #location_str
  if loc_end <= #line_content then
    pcall(vim.api.nvim_buf_set_extmark, buf, ns_id, row, loc_start, {
      end_col = loc_end,
      hl_group = config.hl.grep_line_number or 'LineNr',
      priority = 150,
    })
  end

  -- 3. Separator dimmed
  local sep_start = loc_end
  local sep_end = sep_start + 2
  if sep_end <= #line_content then
    pcall(vim.api.nvim_buf_set_extmark, buf, ns_id, row, sep_start, {
      end_col = sep_end,
      hl_group = 'Comment',
      priority = 150,
    })
  end

  -- 4. Treesitter syntax highlighting for the content portion.
  -- Priority 120: above CursorLine (100) so syntax is visible on cursor line,
  -- below IncSearch match ranges (200) so search matches take precedence.
  local content_start = sep_end
  if item._trimmed_content and item.name then
    local ts_hl = require('fff.treesitter_hl')
    -- Resolve language once per file group (cache on the render context)
    ctx._ts_lang_cache = ctx._ts_lang_cache or {}
    local lang = ctx._ts_lang_cache[item.name]
    if lang == nil then
      lang = ts_hl.lang_from_filename(item.name) or false
      ctx._ts_lang_cache[item.name] = lang
    end
    if lang then
      local highlights = ts_hl.get_line_highlights(item._trimmed_content, lang)
      for _, hl in ipairs(highlights) do
        local hl_start = content_start + hl.col
        local hl_end = content_start + hl.end_col
        if hl_start < #line_content and hl_end <= #line_content then
          pcall(vim.api.nvim_buf_set_extmark, buf, ns_id, row, hl_start, {
            end_col = hl_end,
            hl_group = hl.hl_group,
            priority = 120,
          })
        end
      end
    end
  end

  -- 5. Match ranges highlighted with IncSearch
  -- Use extmarks with priority > cursor line (100) so IncSearch renders
  -- properly on the selected line instead of being overridden by CursorLine.
  if item.match_ranges then
    local leading_ws = item._leading_ws or 0
    for _, range in ipairs(item.match_ranges) do
      local raw_start = range[1] or 0
      local raw_end = range[2] or 0
      local adj_start = raw_start - leading_ws
      local adj_end = raw_end - leading_ws
      if adj_end > 0 then
        adj_start = math.max(0, adj_start)
        local hl_start = content_start + adj_start
        local hl_end = content_start + adj_end
        if hl_start < #line_content and hl_end <= #line_content then
          pcall(vim.api.nvim_buf_set_extmark, buf, ns_id, row, hl_start, {
            end_col = hl_end,
            hl_group = config.hl.grep_match or 'IncSearch',
            priority = 200,
          })
        end
      end
    end
  end

  -- 6. Selection marker (per-occurrence in grep mode)
  if ctx.selected_items then
    local key = string.format('%s:%d:%d', item.path, item.line_number or 0, item.col or 0)
    if ctx.selected_items[key] then
      vim.api.nvim_buf_set_extmark(buf, ns_id, row, 0, {
        sign_text = '▊',
        sign_hl_group = config.hl.selected or 'FFFSelected',
        priority = 1001,
      })
    end
  end
end

--- Render a single item's lines (called by list_renderer's generate_item_lines).
--- Returns 2 lines [header, match] for the first match of a file group,
--- or 1 line [match] for subsequent matches in the same file.
---@param item table Grep match item
---@param ctx table Render context
---@param item_idx number 1-based item index
---@return string[]
function M.render_line(item, ctx, item_idx)
  -- Track file grouping across the render pass via ctx
  -- ctx._grep_last_file is reset each render (ctx is fresh per render_list call)
  local is_new_group = (item.path ~= ctx._grep_last_file)
  ctx._grep_last_file = item.path

  local match_line = render_match_line(item, ctx)

  if is_new_group then
    local header_line = build_group_header(item, ctx)
    item._has_group_header = true
    return { header_line, match_line }
  else
    item._has_group_header = false
    return { match_line }
  end
end

--- Apply highlights for rendered lines (called by list_renderer's apply_all_highlights).
--- line_idx is the 1-based index of the item's LAST line (the match line).
--- If the item has a group header, it's at line_idx - 1.
---@param item table Grep match item
---@param ctx table Render context
---@param item_idx number 1-based item index
---@param buf number Buffer handle
---@param ns_id number Namespace id
---@param line_idx number 1-based line index of the match line
---@param line_content string The rendered match line text
function M.apply_highlights(item, ctx, item_idx, buf, ns_id, line_idx, line_content)
  local row = line_idx - 1 -- 0-based for nvim API

  -- Apply match line highlights
  apply_match_highlights(item, ctx, item_idx, buf, ns_id, row, line_content)

  -- If this item has a group header, highlight it (the line above)
  -- using file_renderer for identical appearance to the file picker list.
  if item._has_group_header then apply_group_header_highlights(item, ctx, buf, ns_id, row - 1) end
end

return M
