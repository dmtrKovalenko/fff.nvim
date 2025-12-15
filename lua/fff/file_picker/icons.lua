local M = {}

local icon_providers = {
  'nvim-web-devicons',
  'mini.icons',
}

M.provider = nil
M.provider_name = nil
M.setup_attempted = false
M.setup_failed = false

function M.setup()
  if M.provider_name then return true end
  if M.setup_failed then return false end

  M.setup_attempted = true

  for _, provider_name in ipairs(icon_providers) do
    local ok, provider = pcall(require, provider_name)
    if ok then
      M.provider = provider
      M.provider_name = provider_name
      return true
    end
  end

  M.setup_failed = true
  vim.notify('FFF Icons: No icon provider found', vim.log.levels.WARN)
  return false
end

--- Get icon for a directory
--- @param dirname string The directory name
--- @return string|nil, string|nil Icon and highlight group (nil if no provider)
function M.get_directory_icon(dirname)
  if not M.setup() then return nil, nil end

  local basename = vim.fn.fnamemodify(dirname, ':t')

  if M.provider_name == 'nvim-web-devicons' then
    if M.provider.get_icon then
      local icon, hl = M.provider.get_icon(basename, nil, { default = true })
      if icon and icon ~= '' and hl then return icon, hl end
    end
  elseif M.provider_name == 'mini.icons' then
    if M.provider.get then
      local icon, hl, is_default = M.provider.get('directory', basename)
      if icon and icon ~= '' and hl then return icon, hl end
    end
  end

  return nil, nil
end

--- Get icon for a file
--- @param filename string The filename
--- @param extension string The file extension (without dot)
--- @param is_directory boolean Whether this is a directory
--- @return string|nil, string|nil Icon and highlight group (nil if no provider)
function M.get_icon(filename, extension, is_directory)
  if not M.setup() then return nil, nil end

  if is_directory then return M.get_directory_icon(filename) end

  if M.provider_name == 'nvim-web-devicons' then
    local icon, hl = M.provider.get_icon(filename, extension, { default = true })
    if icon and icon ~= '' and hl then return icon, hl end
  elseif M.provider_name == 'mini.icons' then
    local icon, hl, _ = M.provider.get('file', filename)
    if icon and icon ~= '' and hl then return icon, hl end
  end

  return nil, nil
end

--- Check if directories are supported by current provider
--- @return boolean True if directory icons are supported
function M.supports_directories() return M.setup() and M.provider_name ~= nil end

--- Get provider info for debugging
--- @return table Provider information
function M.get_provider_info()
  M.setup()
  return {
    name = M.provider_name or 'none',
    available = M.provider ~= nil,
    supports_directories = M.supports_directories(),
  }
end

return M
