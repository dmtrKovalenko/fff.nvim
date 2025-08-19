-- Module based on nvim-lua/plenary.nvim
-- Removed detecting from shebang and modeline.

local filetype = {}

local DATA_DIR = 'data/filetypes'

-- from plenary.path
local os_sep = (function()
  if jit then
    local os = string.lower(jit.os)
    if os ~= 'windows' then
      return '/'
    else
      return '\\'
    end
  else
    return package.config:sub(1, 1)
  end
end)()

local filetype_table = {
  extension = {},
  file_name = {},
}

filetype.add_table = function(new_filetypes)
  local valid_keys = { 'extension', 'file_name' }
  local new_keys = {}

  -- Validate keys
  for k, _ in pairs(new_filetypes) do
    new_keys[k] = true
  end
  for _, k in ipairs(valid_keys) do
    new_keys[k] = nil
  end

  for k, v in pairs(new_keys) do
    error(debug.traceback('Invalid key / value:' .. tostring(k) .. ' / ' .. tostring(v)))
  end

  if new_filetypes.extension then
    filetype_table.extension = vim.tbl_extend('force', filetype_table.extension, new_filetypes.extension)
  end

  if new_filetypes.file_name then
    filetype_table.file_name = vim.tbl_extend('force', filetype_table.file_name, new_filetypes.file_name)
  end
end

filetype.add_file = function(filename)
  local path = string.format('%s/%s.lua', DATA_DIR, filename)
  local filetype_files = vim.api.nvim_get_runtime_file(path, true)

  for _, file in ipairs(filetype_files) do
    local ok, msg = pcall(filetype.add_table, dofile(file))
    if not ok then error('Unable to add file ' .. file .. ':\n' .. msg) end
  end
end

local filename_regex = '[^' .. os_sep .. '].*'
filetype._get_extension_parts = function(filename)
  local current_match = filename:match(filename_regex)
  local possibilities = {}
  while current_match do
    current_match = current_match:match('[^.]%.(.*)')
    if current_match then
      table.insert(possibilities, current_match:lower())
    else
      return possibilities
    end
  end
  return possibilities
end

local done_adding = false
local extend_tbl_with_ext_eq_ft_entries = function()
  if not done_adding then
    if vim.in_fast_event() then return end
    local all_valid_filetypes = vim.fn.getcompletion('', 'filetype')
    for _, v in ipairs(all_valid_filetypes) do
      if not filetype_table.extension[v] then filetype_table.extension[v] = v end
    end
    done_adding = true
    return true
  end
end

filetype.detect_from_extension = function(filepath)
  local exts = filetype._get_extension_parts(filepath)
  for _, ext in ipairs(exts) do
    local match = ext and filetype_table.extension[ext]
    if match then return match end
  end
  if extend_tbl_with_ext_eq_ft_entries() then
    for _, ext in ipairs(exts) do
      local match = ext and filetype_table.extension[ext]
      if match then return match end
    end
  end
  return ''
end

filetype.detect_from_name = function(filepath)
  if filepath then
    filepath = filepath:lower()
    local split_path = vim.split(filepath, os_sep, true)
    local fname = split_path[#split_path]
    local match = filetype_table.file_name[fname]
    if match then return match end
  end
  return ''
end

filetype.detect = function(filepath)
  if type(filepath) ~= string then filepath = tostring(filepath) end

  local match = filetype.detect_from_name(filepath)
  if match ~= '' then return match end

  match = filetype.detect_from_extension(filepath)
  return match
end

filetype.add_file('base')
filetype.add_file('builtin')

return filetype
