local M = {}
local system = require('fff.utils.system')
local fs_utils = require('fff.utils.fs')

local GITHUB_REPO = 'dmtrKovalenko/fff.nvim'

local function get_current_version(plugin_dir, callback)
  vim.system({ 'git', 'rev-parse', '--short', 'HEAD' }, { cwd = plugin_dir }, function(result)
    if result.code ~= 0 or not result.stdout or result.stdout == '' then
      callback(nil)
      return
    end
    callback(result.stdout:gsub('%s+', ''))
  end)
end

local function get_binary_dir(plugin_dir) return plugin_dir .. '/../target/release' end

local function get_binary_path(plugin_dir)
  local binary_dir = get_binary_dir(plugin_dir)
  local extension = system.get_lib_extension()
  return binary_dir .. '/libfff_nvim.' .. extension
end

local function binary_exists(plugin_dir)
  local binary_path = get_binary_path(plugin_dir)
  local stat = vim.uv.fs_stat(binary_path)
  if stat and stat.type == 'file' then return true end

  -- On Windows the rename over a loaded DLL fails, so a verified binary may be
  -- left at binary_path .. '.tmp'. Promote it now that the old session is gone.
  local tmp_path = binary_path .. '.tmp'
  local tmp_stat = vim.uv.fs_stat(tmp_path)
  if tmp_stat and tmp_stat.type == 'file' then
    -- Verify the .tmp is a valid library before promoting it, in case the
    -- process was killed between the loadlib check and the rename attempt
    -- during a previous download, leaving a corrupt or partial .tmp on disk.
    local loader = package.loadlib(tmp_path, 'luaopen_fff_nvim')
    if not loader then
      vim.uv.fs_unlink(tmp_path)
      return false
    end
    local ok = vim.uv.fs_rename(tmp_path, binary_path)
    return ok ~= nil
  end

  return false
end

local function download_file(url, output_path, opts, callback)
  opts = opts or {}

  local dir = vim.fn.fnamemodify(output_path, ':h')
  fs_utils.mkdir_recursive(dir, function(mkdir_ok, mkdir_err)
    if not mkdir_ok then
      callback(false, mkdir_err)
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

    if opts.extra_curl_args then
      for _, arg in ipairs(opts.extra_curl_args) do
        table.insert(curl_args, arg)
      end
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

--- Verify the SHA256 of a file against an expected hash string.
--- @param file_path string
--- @param expected_hash string  lowercase hex SHA256
--- @param callback fun(ok: boolean, err: string|nil)
local function verify_sha256(file_path, expected_hash, callback)
  local cmd
  local sysname = vim.uv.os_uname().sysname:lower()
  if sysname:match('windows') then
    cmd = { 'certutil', '-hashfile', file_path, 'SHA256' }
  elseif sysname == 'darwin' then
    cmd = { 'shasum', '-a', '256', file_path }
  else
    cmd = { 'sha256sum', file_path }
  end

  vim.system(cmd, {}, function(result)
    if result.code ~= 0 then
      local detail = (result.stderr and result.stderr ~= '' and result.stderr)
        or (result.stdout and result.stdout ~= '' and result.stdout)
        or 'unknown error'
      callback(false, 'sha256 command failed: ' .. detail)
      return
    end

    local actual_hash
    if sysname:match('windows') then
      -- Output format of certutil on windows:
      -- SHA256 hash <file_path>:
      -- <hash>
      -- CertUtil: -hashfile command completed successfully.
      actual_hash = (result.stdout or ''):match("hash.-:\r?\n([%x ]+)\r?\nCertUtil: %-hashfile command completed successfully%.")
    else
      actual_hash = (result.stdout or ''):match('^%s*([0-9a-fA-F]+)')
    end
    if not actual_hash then
      callback(false, 'Could not parse sha256 output: ' .. tostring(result.stdout))
      return
    end

    if actual_hash:lower() ~= expected_hash:lower() then
      callback(false, string.format('SHA256 mismatch: expected %s, got %s', expected_hash, actual_hash:lower()))
      return
    end

    callback(true, nil)
  end)
end

local function download_from_github(version, binary_path, opts, callback)
  opts = opts or {}

  local triple = system.get_triple()
  local extension = system.get_lib_extension()
  local binary_name = triple .. '.' .. extension
  local url = string.format('https://github.com/%s/releases/download/%s/%s', GITHUB_REPO, version, binary_name)
  local sha_url = url .. '.sha256'

  vim.schedule(function()
    vim.notify(string.format('Downloading fff.nvim binary for ' .. version), vim.log.levels.INFO)
    vim.notify(string.format('Do not open fff until you see a success notification.'), vim.log.levels.WARN)
  end)

  -- Download to a temp path first so we can verify before replacing the live binary.
  -- If we wrote directly to binary_path and the current process already has the old
  -- library loaded, package.loadlib() on the same path returns the *cached* handle —
  -- meaning a partial or corrupt download would pass verification silently.
  -- Using a distinct temp path forces dlopen to load the new file for real.
  local tmp_path = binary_path .. '.tmp'
  local tmp_sha_path = tmp_path .. '.sha256'

  -- Download the SHA256 checksum file first so we can verify the binary.
  download_file(sha_url, tmp_sha_path, { proxy = opts.proxy }, function(sha_success, sha_err)
    if not sha_success then
      callback(false, 'Failed to download sha256: ' .. (sha_err or 'unknown error'))
      return
    end

    -- Read expected hash (first token on first line)
    local sha_file = io.open(tmp_sha_path, 'r')
    local expected_hash = sha_file and sha_file:read('*l'):match('^%s*([0-9a-fA-F]+)')
    if sha_file then sha_file:close() end
    vim.uv.fs_unlink(tmp_sha_path)

    if not expected_hash or #expected_hash ~= 64 then
      callback(false, 'Invalid sha256 file contents')
      return
    end

    download_file(url, tmp_path, {
      proxy = opts.proxy,
      extra_curl_args = opts.extra_curl_args,
    }, function(success, err)
      if not success then
        vim.uv.fs_unlink(tmp_path)
        callback(false, err)
        return
      end

      -- Verify integrity before doing anything else with the binary.
      verify_sha256(tmp_path, expected_hash, function(hash_ok, hash_err)
        vim.schedule(function()
          if not hash_ok then
            vim.uv.fs_unlink(tmp_path)
            callback(false, 'Binary integrity check failed: ' .. (hash_err or 'unknown error'))
            return
          end

          -- Verify the NEW binary (temp path is not yet loaded by this process,
          -- so dlopen actually loads and validates the downloaded file).
          -- Note: package.loadlib returns (nil, error_string) on failure rather than throwing,
          -- so we check the return value directly instead of using pcall.
          local loader, load_err = package.loadlib(tmp_path, 'luaopen_fff_nvim')

          if not loader then
            vim.uv.fs_unlink(tmp_path)
            callback(false, 'Downloaded binary is not valid: ' .. (load_err or 'unknown error'))
            return
          end

          -- Atomically replace the live binary only after successful verification.
          -- On Windows the old .dll may be locked by the current process, so rename can
          -- fail if fff is already loaded. In that case, leave the verified .tmp on disk
          -- so the next Neovim start can pick it up automatically.
          local rename_ok, rename_err = vim.uv.fs_rename(tmp_path, binary_path)
          if not rename_ok then
            if vim.uv.os_uname().sysname:lower():match('windows') then
              vim.notify(
                'fff.nvim binary downloaded to '
                  .. tmp_path
                  .. '.\nThe live binary is locked by the current session — please restart Neovim to apply the update.',
                vim.log.levels.WARN
              )
              callback(true, nil)
            else
              vim.uv.fs_unlink(tmp_path)
              callback(false, 'Failed to install binary: ' .. (rename_err or 'unknown error'))
            end
            return
          end

          vim.notify('fff.nvim binary downloaded successfully!', vim.log.levels.INFO)
          callback(true, nil)
        end)
      end) -- verify_sha256
    end) -- binary download_file
  end) -- sha download_file
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
        vim.schedule(
          function()
            vim.notify('Failed to download fff.nvim binary: ' .. (err or 'unknown error'), vim.log.levels.ERROR)
          end
        )
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
