local M = {}

local uv = vim and vim.uv or require('luv')

-- Get the system triple (target triple for the current platform)
function M.get_triple()
  local os_name = uv.os_uname().sysname:lower()
  local arch = uv.os_uname().machine:lower()

  -- Normalize OS name
  if os_name == 'darwin' then
    os_name = 'apple-darwin'
  elseif os_name == 'linux' then
    -- Detect if we're on musl or glibc
    local handle = io.popen('ldd --version 2>&1')
    if handle then
      local output = handle:read('*a')
      handle:close()
      if output and output:match('musl') then
        os_name = 'unknown-linux-musl'
      else
        os_name = 'unknown-linux-gnu'
      end
    else
      os_name = 'unknown-linux-gnu'
    end
  elseif os_name:match('windows') or os_name:match('mingw') or os_name:match('msys') then
    os_name = 'pc-windows-msvc'
  end

  -- Normalize architecture
  if arch == 'x86_64' or arch == 'amd64' then
    arch = 'x86_64'
  elseif arch == 'aarch64' or arch == 'arm64' then
    arch = 'aarch64'
  elseif arch:match('^arm') then
    arch = 'arm'
  end

  return arch .. '-' .. os_name
end

-- Get the library extension for the current platform
function M.get_lib_extension()
  local os_name = uv.os_uname().sysname:lower()
  if os_name == 'darwin' then
    return 'dylib'
  elseif os_name:match('windows') or os_name:match('mingw') or os_name:match('msys') then
    return 'dll'
  else
    return 'so'
  end
end

return M
