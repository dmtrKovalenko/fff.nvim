local M = {}

local icon_providers = {
  'nvim-web-devicons',
  'mini.icons',
}

M.provider = nil
M.provider_name = nil
M.setup_attempted = false
M.setup_failed = false

local directory_configs = {
  ['nvim-web-devicons'] = {
    default = { icon = '󰉋', hl = 'DevIconDefault' },
    open = { icon = '󰝰', hl = 'DevIconDefault' },
    closed = { icon = '󰉋', hl = 'DevIconDefault' },
    git = { icon = '', hl = 'DevIconGitIgnore' },
    node_modules = { icon = '', hl = 'DevIconNodeModules' },
    hidden = { icon = '󰘓', hl = 'DevIconDefault' },
  },
  ['mini.icons'] = {
    default = { icon = '󰉋', color = '#7aa2f7' },
    open = { icon = '󰝰', color = '#7aa2f7' },
    closed = { icon = '󰉋', color = '#7aa2f7' },
    git = { icon = '', color = '#e24329' },
    node_modules = { icon = '', color = '#8cc84b' },
    hidden = { icon = '󰘓', color = '#6d8086' },
  },
}

-- Special directory names and their icons
local special_directories = {
  ['.git'] = 'git',
  ['node_modules'] = 'node_modules',
  ['.vscode'] = 'hidden',
  ['.idea'] = 'hidden',
  ['.cache'] = 'hidden',
  ['.config'] = 'hidden',
  ['__pycache__'] = 'hidden',
  ['.pytest_cache'] = 'hidden',
  ['target'] = 'hidden',
  ['dist'] = 'hidden',
  ['build'] = 'hidden',
  ['out'] = 'hidden',
  ['.next'] = 'hidden',
  ['.nuxt'] = 'hidden',
  ['coverage'] = 'hidden',
}

M.highlight_cache = {}

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
  vim.notify('FFF Icons: No icon provider found. Please install nvim-web-devicons or mini.icons', vim.log.levels.WARN)
  return false
end

--- Get icon for a directory
--- @param dirname string The directory name
--- @return string, string Icon and color/highlight
function M.get_directory_icon(dirname)
  if not M.setup() then
    return '󰉋', '#7aa2f7' -- Default folder icon if no provider
  end

  local dir_type = 'default'
  local basename = vim.fn.fnamemodify(dirname, ':t')

  if special_directories[basename] then
    dir_type = special_directories[basename]
  elseif basename:match('^%.') then
    dir_type = 'hidden'
  end

  local config = directory_configs[M.provider_name]
  if not config or not config[dir_type] then dir_type = 'default' end

  local icon_data = config[dir_type]

  if M.provider_name == 'nvim-web-devicons' then
    -- For nvim-web-devicons, try to get the actual icon first
    if M.provider.get_icon then
      local provider_icon, provider_hl = M.provider.get_icon(basename, nil, { default = false })
      if provider_icon and provider_icon ~= '' then
        return provider_icon, M.resolve_color(provider_hl or icon_data.hl)
      end
    end

    -- Use our configured icon
    return icon_data.icon, M.resolve_color(icon_data.hl)
  elseif M.provider_name == 'mini.icons' then
    -- For mini.icons, try to get directory-specific icon
    if M.provider.get then
      local provider_data = M.provider.get('directory', basename)
      if provider_data and provider_data.glyph and provider_data.glyph ~= '' then
        return provider_data.glyph, M.get_color_from_highlight(provider_data.hl)
      end
    end

    -- Use our configured icon
    return icon_data.icon, icon_data.color
  end

  -- Fallback (shouldn't reach here)
  return '󰉋', '#7aa2f7'
end

--- Get icon for a file
--- @param filename string The filename
--- @param extension string The file extension (without dot)
--- @param is_directory boolean Whether this is a directory
--- @return string, string Icon and color
function M.get_icon(filename, extension, is_directory)
  if not M.setup() then
    if is_directory then
      return '󰉋', '#7aa2f7'
    else
      return '󰈙', '#6d8086'
    end
  end

  if is_directory then return M.get_directory_icon(filename) end

  local icon, color_or_hl

  if M.provider_name == 'nvim-web-devicons' then
    icon, color_or_hl = M.provider.get_icon(filename, extension, { default = true })
    if icon and icon ~= '' then return icon, M.resolve_color(color_or_hl) end
  elseif M.provider_name == 'mini.icons' then
    local icon_data = M.provider.get('file', filename)
    if icon_data and icon_data.glyph and icon_data.glyph ~= '' then
      return icon_data.glyph, M.get_color_from_highlight(icon_data.hl)
    end
  end

  return '󰈙', '#6d8086'
end

--- Get folder icon (kept for compatibility)
--- @return string, string Icon and color
function M.get_folder_icon() return M.get_directory_icon('folder') end

--- Resolve color from highlight group or hex
--- @param color_or_hl string|nil Color hex or highlight group name
--- @return string Hex color
function M.resolve_color(color_or_hl)
  if not color_or_hl or color_or_hl == '' then return '#6d8086' end

  -- If it's already a hex color, return as-is
  if color_or_hl:match('^#%x%x%x%x%x%x$') then return color_or_hl end

  -- Try to resolve as highlight group
  return M.get_color_from_highlight(color_or_hl)
end

--- Get hex color from highlight group
--- @param hl_group string Highlight group name
--- @return string Hex color
function M.get_color_from_highlight(hl_group)
  if not hl_group or hl_group == '' then return '#6d8086' end

  local ok, hl = pcall(vim.api.nvim_get_hl, 0, { name = hl_group })
  if ok and hl and hl.fg then return string.format('#%06x', hl.fg) end

  return '#6d8086' -- Fallback color
end

--- Get icon with display formatting and highlight group creation
--- @param filename string The filename
--- @param extension string The file extension (without dot)
--- @param is_directory boolean Whether this is a directory
--- @return string, string Icon and highlight group name
function M.get_icon_display(filename, extension, is_directory)
  local icon, color = M.get_icon(filename, extension, is_directory)
  local hl_group = M.create_icon_highlight(color)
  return icon, hl_group
end

--- Create or get cached highlight group for icon color
--- @param color string Hex color
--- @return string Highlight group name
function M.create_icon_highlight(color)
  if not color or color == '' then color = '#6d8086' end
  if not color:match('^#%x%x%x%x%x%x$') then color = M.resolve_color(color) end
  local hl_name = 'FFFIcon' .. color:gsub('#', ''):upper()

  if M.highlight_cache[hl_name] then return hl_name end

  local ok = pcall(vim.api.nvim_set_hl, 0, hl_name, { fg = color })
  if not ok then
    color = '#6d8086'
    hl_name = 'FFFIcon6D8086'
    vim.api.nvim_set_hl(0, hl_name, { fg = color })
  end

  M.highlight_cache[hl_name] = true
  return hl_name
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
