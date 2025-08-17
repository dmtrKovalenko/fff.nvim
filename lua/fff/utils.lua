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

--- Safely resolve a config value that can be either a static value or a function
--- @param config_value any The config value (can be function or static value)
--- @param terminal_width number Terminal width for function calls
--- @param terminal_height number Terminal height for function calls
--- @param validator function Function to validate the result
--- @param fallback any Fallback value if function fails or returns invalid value
--- @param error_context string Context for error messages
--- @return any The resolved and validated value
function M.resolve_config_value(config_value, terminal_width, terminal_height, validator, fallback, error_context)
  if type(config_value) == 'function' then
    local success, result = pcall(config_value, terminal_width, terminal_height)

    if success and validator(result) then
      return result
    else
      if not success then
        vim.notify('FFF: Error in ' .. error_context .. ' function: ' .. tostring(result), vim.log.levels.WARN)
      end
      return fallback
    end
  else
    return config_value
  end
end

--- Validate numeric ratio (0 < value <= 1)
--- @param value any Value to validate
--- @return boolean True if valid numeric ratio
function M.is_valid_ratio(value) return type(value) == 'number' and value > 0 and value <= 1 end

--- Validate position string
--- @param value any Value to validate
--- @param values table List of valid values strings
--- @return boolean True if valid position
function M.is_one_of(value, values)
  if type(value) ~= 'string' then return false end
  for _, pos in ipairs(values) do
    if value == pos then return true end
  end
  return false
end

return M
