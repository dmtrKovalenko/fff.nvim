local download = require('fff.download')

--- @return string
local function get_lib_extension()
  if jit.os:lower() == 'mac' or jit.os:lower() == 'osx' then return '.dylib' end
  if jit.os:lower() == 'windows' then return '.dll' end
  return '.so'
end

-- search for the lib in the /target/release directory with and without the lib prefix
-- since MSVC doesn't include the prefix
local info = debug.getinfo(1, 'S')
local base_path = info and info.source and info.source:match('@?(.*/)') or ''

-- Fallback: if base_path is nil, try to determine from current file path
if not base_path or base_path == '' then
  base_path = vim.fn.fnamemodify(vim.fn.resolve(vim.fn.expand('<sfile>:p')), ':h') .. '/'
end

local paths = {
  download.get_binary_cpath_component(),
  base_path .. '../../../target/release/lib?' .. get_lib_extension(),
  base_path .. '../../../target/release/?' .. get_lib_extension(),
}

local cargo_target_dir = os.getenv('CARGO_TARGET_DIR')
if cargo_target_dir then
  table.insert(paths, cargo_target_dir .. '/release/lib?' .. get_lib_extension())
  table.insert(paths, cargo_target_dir .. '/release/?' .. get_lib_extension())
end

-- Instead of using require (which can find the wrong lib due to cpath pollution),
-- load the library directly from the first valid path we find
local function try_load_library()
  for _, path_pattern in ipairs(paths) do
    local actual_path = path_pattern:gsub('%?', 'fff_nvim')
    local stat = vim.uv.fs_stat(actual_path)
    if stat and stat.type == 'file' then
      local loader, err = package.loadlib(actual_path, 'luaopen_fff_nvim')
      if err then return nil, string.format('Error loading library from %s: %s', actual_path, err) end
      if loader then return loader() end
    end
  end
  return nil, 'No valid library found in any search path'
end

local backend, load_err = try_load_library()
if not backend or load_err then
  local err_msg = string.format(
    'Failed to load fff rust backend.\nError: %s\nSearched paths:\n%s\nMake sure binary exists or make it exists using \n `:lua require("fff.download").download_or_build_binary()`\nor\n`cargo build --release`\n(and rerun neovim after)',
    tostring(load_err),
    vim.inspect(paths)
  )

  error(err_msg)
end

return backend
