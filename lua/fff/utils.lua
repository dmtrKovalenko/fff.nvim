local M = {}

--- Format file size into human-readable string
--- @param size number File size in bytes
--- @return string Formatted size string (e.g., "1.2 KB", "3.4 MB")
function M.format_file_size(size)
  if not size or size < 0 then return 'Unknown' end

  if size < 1024 then
    return string.format('%d B', size)
  elseif size < 1024 * 1024 then
    return string.format('%.1f KB', size / 1024)
  elseif size < 1024 * 1024 * 1024 then
    return string.format('%.1f MB', size / (1024 * 1024))
  else
    return string.format('%.1f GB', size / (1024 * 1024 * 1024))
  end
end

return M

