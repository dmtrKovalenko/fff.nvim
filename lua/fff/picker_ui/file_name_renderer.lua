local M = {}

local path_separator = package.config:sub(1, 1)

--- @class FffFileNameLayout
--- @field text string Icon + name section of the rendered line
--- @field filename string File name as returned by `ctx.format_file_display`
--- @field dir_path string Shortened directory, '' when the file sits at the root
--- @field filename_col number 0-based byte column of the file name inside `text`
--- @field dir_col number 0-based byte column of `dir_path` inside `text`
--- @field dir_end_col number Byte column past `dir_path`, separator included when path first
--- @field path_first boolean

--- True when file names render as `dir/name` instead of `name dir`.
--- @param config FffConfig|nil
--- @return boolean
function M.is_path_first(config) return (config and config.layout and config.layout.show_path_first) == true end

--- Build the icon + name section of a file line together with its byte offsets.
--- @param item FileItem File item from Rust
--- @param ctx ListRenderContext Render context with all state
--- @param icon string|nil Already resolved file icon
--- @return FffFileNameLayout
function M.build(item, ctx, icon)
  local path_first = M.is_path_first(ctx.config)
  local icon_width = icon and (vim.fn.strdisplaywidth(icon) + 1) or 0
  -- name-first needs a floor so the directory column does not jump around,
  -- path-first has nothing to align and takes whatever the window gives
  local available_width = math.max(ctx.max_path_width - icon_width, path_first and 0 or 40)
  local filename, dir_path = ctx.format_file_display(item, available_width)

  local prefix = icon and (icon .. ' ') or ''
  local prefix_len = #prefix
  if path_first then
    local separator = dir_path ~= '' and path_separator or ''
    local dir_end_col = prefix_len + #dir_path + #separator
    return {
      text = prefix .. dir_path .. separator .. filename,
      filename = filename,
      dir_path = dir_path,
      filename_col = dir_end_col,
      dir_col = prefix_len,
      dir_end_col = dir_end_col,
      path_first = true,
    }
  end

  local dir_col = prefix_len + #filename + 1
  return {
    text = prefix .. filename .. ' ' .. dir_path,
    filename = filename,
    dir_path = dir_path,
    filename_col = prefix_len,
    dir_col = dir_col,
    dir_end_col = dir_col + #dir_path,
    path_first = false,
  }
end

--- Map fuzzy match ranges over `item.relative_path` onto line byte columns.
--- Segments are `{ source_start, source_end, target_col }` triples.
--- @param item FileItem File item from Rust
--- @param layout FffFileNameLayout Layout returned by `M.build`
--- @return number[][]
function M.fuzzy_segments(item, layout)
  local rel_path = item.relative_path or ''
  if type(rel_path) ~= 'string' then rel_path = tostring(rel_path) end

  local filename_rel_start = math.max(0, #rel_path - #layout.filename)
  local segments = { { filename_rel_start, filename_rel_start + #layout.filename, layout.filename_col } }

  local parent_dir = vim.fn.fnamemodify(rel_path, ':h')
  if parent_dir == '.' then parent_dir = '' end

  -- a shortened directory breaks the byte mapping, only map an intact one
  if parent_dir ~= '' and layout.dir_path == parent_dir then
    local dir_source_end = layout.path_first and filename_rel_start or #parent_dir
    segments[#segments + 1] = { 0, dir_source_end, layout.dir_col }
  end

  return segments
end

return M
