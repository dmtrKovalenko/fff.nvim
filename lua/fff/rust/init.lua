local download = require('fff.download')

--- @return string
local function get_lib_extension()
  if jit.os:lower() == 'mac' or jit.os:lower() == 'osx' then return '.dylib' end
  if jit.os:lower() == 'windows' then return '.dll' end
  return '.so'
end

---@param bin_path string
---@return string
local function get_binary_cpath(bin_path)
    local bin_dir = vim.fn.fnamemodify(bin_path, ':h')
    local bin_file = vim.fn.fnamemodify(bin_path, ':t')
    local cfile = bin_file:gsub('fff_nvim', '?')
    return bin_dir .. '/' .. cfile
end

-- search for the lib in the /target/release directory with and without the lib prefix
-- since MSVC doesn't include the prefix
local base_path = debug.getinfo(1).source:match('@?(.*/)')
local binary_cpath = get_binary_cpath(download.get_binary_path())

local paths = {
  binary_cpath,
  base_path .. '../../../target/release/lib?' .. get_lib_extension(),
  base_path .. '../../../target/release/?' .. get_lib_extension(),
}

local cargo_target_dir = os.getenv('CARGO_TARGET_DIR')
if cargo_target_dir then
  table.insert(paths, cargo_target_dir .. '/release/lib?' .. get_lib_extension())
  table.insert(paths, cargo_target_dir .. '/release/?' .. get_lib_extension())
end

package.cpath = package.cpath .. ';' .. table.concat(paths, ';')

local ok, backend = pcall(require, 'fff_nvim')
if not ok then
  error('Failed to load fff rust backend. Make sure that it has been downloaded or built with `cargo build --release`')
end

return backend
