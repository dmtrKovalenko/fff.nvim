local M = {}
local system = require('fff.utils.system')
local uv = vim and vim.uv or require('luv')

local GITHUB_REPO = 'madmaxieee/fff.nvim'

local function get_current_version(plugin_dir, callback)
  vim.system({ 'git', 'rev-parse', '--short', 'HEAD' }, { cwd = plugin_dir }, function(result)
    if result.code ~= 0 or not result.stdout or result.stdout == '' then
      callback(nil)
      return
    end
    callback(result.stdout:gsub('%s+', ''))
  end)
end

local function get_binary_dir(plugin_dir) return plugin_dir .. '/../target' end

local function get_binary_path(plugin_dir)
  local binary_dir = get_binary_dir(plugin_dir)
  local extension = system.get_lib_extension()
  return binary_dir .. '/libfff_nvim.' .. extension
end

local function binary_exists(plugin_dir)
  local binary_path = get_binary_path(plugin_dir)
  local stat = uv.fs_stat(binary_path)
  return stat and stat.type == 'file'
end

local function download_file(url, output_path, opts, callback)
  opts = opts or {}

  local dir = vim.fn.fnamemodify(output_path, ':h')
  uv.fs_mkdir(dir, 493, function(err) -- 493 = 0755 octal
    if err and not err:match('EEXIST') then
      callback(false, 'Failed to create directory: ' .. err)
      return
    end

    local curl_args = {
      'curl',
      '--fail',
      '--location',
      '--silent',
      '--show-error',
      '--output',
      output_path,
    }

    if opts.proxy then
      table.insert(curl_args, '--proxy')
      table.insert(curl_args, opts.proxy)
    end

    table.insert(curl_args, url)
    vim.system(curl_args, {}, function(result)
      if result.code ~= 0 then
        callback(false, 'Failed to download: ' .. (result.stderr or 'unknown error'))
        return
      end
      callback(true, nil)
    end)
  end)
end

local function download_from_github(version, binary_path, opts, callback)
  opts = opts or {}

  local triple = system.get_triple()
  local extension = system.get_lib_extension()
  local binary_name = triple .. '.' .. extension
  local url = string.format('https://github.com/%s/releases/download/%s/%s', GITHUB_REPO, version, binary_name)
  vim.schedule(function()
    vim.notify(string.format('Downloading fff.nvim binary for ' .. version), vim.log.levels.INFO)
    vim.notify(string.format('Do not open fff until you see a success notification.'), vim.log.levels.WARN)
  end)

  download_file(url, binary_path, {
    proxy = opts.proxy,
    extra_curl_args = opts.extra_curl_args,
  }, function(success, err)
    if not success then
      callback(false, err)
      return
    end

    -- Verify the binary can be loaded
    local ok, err_msg = pcall(function() package.loadlib(binary_path, 'luaopen_fff_nvim') end)

    if not ok then
      uv.fs_unlink(binary_path)
      callback(false, 'Downloaded binary is not valid: ' .. (err_msg or 'unknown error'))
      return
    end

    vim.schedule(function() vim.notify('fff.nvim binary downloaded successfully!', vim.log.levels.INFO) end)
    callback(true, nil)
  end)
end

function M.ensure_downloaded(opts, callback)
  opts = opts or {}
  local plugin_dir = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':h:h')

  if binary_exists(plugin_dir) and not opts.force then
    callback(true, nil)
    return
  end

  local function on_version(target_version)
    if not target_version then
      callback(false, 'Could not determine target version')
      return
    end

    local binary_path = get_binary_path(plugin_dir)
    download_from_github(target_version, binary_path, opts, callback)
  end

  if opts.version then
    on_version(opts.version)
  else
    get_current_version(plugin_dir, on_version)
  end
end

function M.download_binary(callback)
  M.ensure_downloaded({ force = true }, function(success, err)
    if not success then
      if callback then
        callback(false, err)
      else
        error('Failed to download fff.nvim binary: ' .. (err or 'unknown error'))
      end
      return
    end
    if callback then callback(true, nil) end
  end)
end

function M.build_binary(callback)
  local plugin_dir = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':h:h')
  local has_rustup = vim.fn.executable('rustup') == 1
  if not has_rustup then
    callback(
      false,
      'rustup is not found. It is required to build the fff.nvim binary. Install it from https://rustup.rs/'
    )
    return
  end

  vim.system({ 'cargo', 'build', '--release' }, { cwd = plugin_dir }, function(result)
    if result.code ~= 0 then
      callback(false, 'Failed to build rust binary: ' .. (result.stderr or 'unknown error'))
      return
    end
    callback(true, nil)
  end)
end

function M.download_or_build_binary()
  M.ensure_downloaded({ force = true }, function(download_success, download_error)
    if download_success then return end

    vim.schedule(
      function()
        vim.notify(
          'Error downloading binary: ' .. (download_error or 'unknown error') .. '\nTrying cargo build --release\n',
          vim.log.levels.WARN
        )
      end
    )

    M.build_binary(function(build_success, build_error)
      if not build_success then
        error('Failed to build fff.nvim binary. Build error: ' .. (build_error or 'unknown error'))
      else
        vim.schedule(function() vim.notify('fff.nvim binary built successfully!', vim.log.levels.INFO) end)
      end
    end)
  end)
end

function M.get_binary_path()
  local plugin_dir = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':h:h')
  return get_binary_path(plugin_dir)
end

function M.get_binary_cpath_component()
  local plugin_dir = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':h:h')
  local binary_dir = get_binary_dir(plugin_dir)
  local extension = system.get_lib_extension()
  return binary_dir .. '/lib?.' .. extension
end

return M
