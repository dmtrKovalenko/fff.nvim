--- List Renderer
--- Handles all list rendering: line generation, virtual rows, bottom padding,
--- buffer writes, cursor positioning, and highlight application.
---
--- Virtual rows (combo headers, grep file group headers) are decorations that
--- belong to buffer rendering, NOT to the data model. The cursor and selection
--- always operate on the items array (1-based indices), never on buffer lines.
---
--- Pagination is unaffected: Rust returns N items per page. The renderer may
--- produce N + K buffer lines (where K = number of virtual header rows), but
--- the page_size contract with Rust stays item-based.
---
--- Selection always operates on item.path keys. Virtual rows have no identity
--- of their own — they derive from the item they belong to.
local M = {}

--- @class ListRenderContext
--- @field config table User configuration
--- @field items table[] Array of data items to render
--- @field cursor number Current cursor position (1-based index into items)
--- @field win_height number Window height in lines
--- @field win_width number Window width in columns
--- @field max_path_width number Actual text area width (excluding signcolumn)
--- @field debug_enabled boolean Whether debug mode shows scores
--- @field prompt_position string 'top' or 'bottom'
--- @field has_combo boolean Whether combo boost is active
--- @field combo_header_line string|nil Formatted combo header line
--- @field combo_header_text_len number|nil Length of combo header text
--- @field combo_item_index number|nil Index of item with combo (usually 1)
--- @field display_start number Start index for displayed items (1)
--- @field display_end number End index for displayed items (#items)
--- @field iter_start number Iteration start
--- @field iter_end number Iteration end
--- @field iter_step number Iteration step (1 or -1)
--- @field renderer table|nil Custom renderer with render_line/apply_highlights
--- @field query string Current search query
--- @field selected_files table<string, boolean> Selected file paths set
--- @field mode string|nil Current mode (nil or 'grep')
--- @field format_file_display function Helper for formatting file display

--- @class ItemLineMapping
--- @field first number First buffer line (1-based) this item occupies
--- @field last number Last buffer line (1-based) — the selectable content line
--- @field virtual_count number Number of virtual (header) lines before the content line

--- @class ListRenderResult
--- @field lines string[] All buffer lines (including virtual rows and padding)
--- @field item_to_lines table<number, ItemLineMapping> Maps item index -> line range
--- @field padding_offset number Number of empty lines prepended for bottom prompt
--- @field total_content_lines number Lines before padding was applied

--- Generate all display lines from items using the renderer.
--- Each item may produce 1 or more lines (virtual header + content).
--- When cross-mode suggestions are active, a suggestion banner is prepended
--- (for top prompt) or appended (for bottom prompt) so it always appears
--- above the suggestion items visually.
--- @param ctx ListRenderContext
--- @return string[] lines Array of line strings
--- @return table<number, ItemLineMapping> item_to_lines
local function generate_item_lines(ctx)
  local lines = {}
  local item_to_lines = {}

  -- Cross-mode suggestion header: rendered above items visually.
  -- For top prompt that means before items; for bottom prompt after items
  -- (because bottom prompt iterates in reverse).
  local suggestion_header_lines = {}
  local has_suggestion_header = ctx.suggestion_source ~= nil and #ctx.items > 0
  if has_suggestion_header then
    table.insert(suggestion_header_lines, '')
    local mode_label = ctx.suggestion_source == 'grep' and 'content matches' or 'file name matches'
    table.insert(suggestion_header_lines, '  No results found. Suggested ' .. mode_label .. ':')
    table.insert(suggestion_header_lines, '')
  end

  -- For top prompt: suggestion header goes before items
  if has_suggestion_header and ctx.prompt_position ~= 'bottom' then
    for _, hline in ipairs(suggestion_header_lines) do
      table.insert(lines, hline)
    end
  end

  local renderer = ctx.renderer
  if not renderer then renderer = require('fff.file_renderer') end

  for i = ctx.iter_start, ctx.iter_end, ctx.iter_step do
    local item = ctx.items[i]
    local item_start_line = #lines + 1

    -- Renderer returns 1+ lines: virtual headers first, content line last.
    -- This contract is shared by file_renderer (combo header) and
    -- grep_renderer (file group header).
    local item_lines = renderer.render_line(item, ctx, i)

    for _, line in ipairs(item_lines) do
      table.insert(lines, line)
    end

    local item_end_line = #lines
    local virtual_count = item_end_line - item_start_line -- 0 if single line, 1 if header + content

    item_to_lines[i] = {
      first = item_start_line,
      last = item_end_line,
      virtual_count = virtual_count,
    }
  end

  -- For bottom prompt: suggestion header goes after items (appears above visually)
  if has_suggestion_header and ctx.prompt_position == 'bottom' then
    for _, hline in ipairs(suggestion_header_lines) do
      table.insert(lines, hline)
    end
  end

  return lines, item_to_lines
end

--- Apply bottom padding: prepend empty lines so content sits at the bottom.
--- Adjusts all line indices in item_to_lines accordingly.
--- @param lines string[] Lines array (mutated)
--- @param item_to_lines table<number, ItemLineMapping> Mapping (mutated)
--- @param ctx ListRenderContext
--- @return number padding_offset Number of empty lines prepended
local function apply_bottom_padding(lines, item_to_lines, ctx)
  if ctx.prompt_position ~= 'bottom' then return 0 end

  local total_content_lines = #lines
  local empty_lines_needed = math.max(0, ctx.win_height - total_content_lines)

  if empty_lines_needed > 0 then
    -- Prepend empty lines
    for _ = empty_lines_needed, 1, -1 do
      table.insert(lines, 1, string.rep(' ', ctx.win_width + 5))
    end

    -- Shift all line indices
    for i = ctx.display_start, ctx.display_end do
      if item_to_lines[i] then
        item_to_lines[i].first = item_to_lines[i].first + empty_lines_needed
        item_to_lines[i].last = item_to_lines[i].last + empty_lines_needed
      end
    end
  end

  return empty_lines_needed
end

--- Write lines to the buffer and position the cursor on the correct line.
--- The cursor always targets the content line (last) of the current item,
--- never a virtual header line.
--- @param lines string[]
--- @param item_to_lines table<number, ItemLineMapping>
--- @param ctx ListRenderContext
--- @param list_buf number Buffer handle
--- @param list_win number Window handle
--- @param ns_id number Namespace id
local function update_buffer_and_cursor(lines, item_to_lines, ctx, list_buf, list_win, ns_id)
  -- Resolve cursor to a buffer line — always the content line (last), not virtual rows
  local cursor_line = 0
  if #ctx.items > 0 and ctx.cursor >= 1 and ctx.cursor <= #ctx.items then
    local cursor_item = item_to_lines[ctx.cursor]
    if cursor_item then cursor_line = cursor_item.last end
  end

  vim.api.nvim_buf_set_option(list_buf, 'modifiable', true)
  vim.api.nvim_buf_set_lines(list_buf, 0, -1, false, lines)
  vim.api.nvim_buf_set_option(list_buf, 'modifiable', false)

  vim.api.nvim_buf_clear_namespace(list_buf, ns_id, 0, -1)

  if #ctx.items > 0 and cursor_line > 0 and cursor_line <= #lines then
    vim.api.nvim_win_set_cursor(list_win, { cursor_line, 0 })
  end
end

--- Apply highlights for all items using the renderer's apply_highlights.
--- For each item, we pass the content line (last) to the renderer.
--- Renderers that emit virtual rows (grep_renderer) handle their own
--- header highlights internally via the item._has_group_header flag.
--- @param lines string[]
--- @param item_to_lines table<number, ItemLineMapping>
--- @param ctx ListRenderContext
--- @param list_buf number
--- @param ns_id number
local function apply_all_highlights(lines, item_to_lines, ctx, list_buf, ns_id)
  local renderer = ctx.renderer
  if not renderer then renderer = require('fff.file_renderer') end

  for i = ctx.display_start, ctx.display_end do
    local item = ctx.items[i]
    local item_lines = item_to_lines[i]
    if not item_lines then goto continue end

    -- The content line is always the last line in the mapping
    local line_idx = item_lines.last
    local line_content = lines[line_idx]

    if not line_content then goto continue end

    renderer.apply_highlights(item, ctx, i, list_buf, ns_id, line_idx, line_content)
    ::continue::
  end
end

--- Render the full item list into the buffer.
--- This is the main entry point — replaces the inline rendering in picker_ui.
---
--- @param ctx ListRenderContext Render context built by picker_ui
--- @param list_buf number List buffer handle
--- @param list_win number List window handle
--- @param ns_id number Highlight namespace
--- @return table<number, ItemLineMapping> item_to_lines for combo/scrollbar use
function M.render(ctx, list_buf, list_win, ns_id)
  local lines, item_to_lines = generate_item_lines(ctx)

  apply_bottom_padding(lines, item_to_lines, ctx)
  update_buffer_and_cursor(lines, item_to_lines, ctx, list_buf, list_win, ns_id)

  if #ctx.items > 0 then apply_all_highlights(lines, item_to_lines, ctx, list_buf, ns_id) end

  -- Highlight the suggestion header lines (if present)
  if ctx.suggestion_source and #ctx.items > 0 then
    local suggestion_hl = ctx.config.hl.suggestion_header or 'WarningMsg'
    for i = 0, #lines - 1 do
      local line = lines[i + 1]
      if line and line:match('^%s+No results found') then
        pcall(vim.api.nvim_buf_add_highlight, list_buf, ns_id, suggestion_hl, i, 0, -1)
      end
    end
  end

  return item_to_lines
end

--- Get the buffer line for an item's content (selectable) line.
--- Used by picker_ui for cursor positioning after navigation.
--- @param item_to_lines table<number, ItemLineMapping>
--- @param item_index number 1-based item index
--- @return number|nil line 1-based buffer line, or nil if item not mapped
function M.get_content_line(item_to_lines, item_index)
  local mapping = item_to_lines[item_index]
  if not mapping then return nil end
  return mapping.last
end

--- Get the buffer line for an item's first line (may be a virtual header).
--- Used by combo_renderer for overlay positioning.
--- @param item_to_lines table<number, ItemLineMapping>
--- @param item_index number 1-based item index
--- @return number|nil line 1-based buffer line, or nil if item not mapped
function M.get_first_line(item_to_lines, item_index)
  local mapping = item_to_lines[item_index]
  if not mapping then return nil end
  return mapping.first
end

--- Check if an item has virtual (header) rows.
--- @param item_to_lines table<number, ItemLineMapping>
--- @param item_index number 1-based item index
--- @return boolean
function M.has_virtual_rows(item_to_lines, item_index)
  local mapping = item_to_lines[item_index]
  if not mapping then return false end
  return mapping.virtual_count > 0
end

--- Count total buffer lines an item occupies (content + virtual).
--- @param item_to_lines table<number, ItemLineMapping>
--- @param item_index number 1-based item index
--- @return number
function M.get_line_count(item_to_lines, item_index)
  local mapping = item_to_lines[item_index]
  if not mapping then return 0 end
  return mapping.last - mapping.first + 1
end

return M
