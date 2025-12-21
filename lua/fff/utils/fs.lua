local M = {}

function M.mkdir_recursive(path, callback)
  vim.uv.fs_stat(path, function(err, stat)
    if not err and stat then
      callback(true, nil)
      return
    end

    local parent = vim.fn.fnamemodify(path, ':h')
    if parent == path or parent == '' or parent == '.' then
      callback(false, 'Cannot create root directory')
      return
    end

    M.mkdir_recursive(parent, function(parent_ok, parent_err)
      if not parent_ok then
        callback(false, parent_err)
        return
      end

      vim.uv.fs_mkdir(path, 493, function(mkdir_err) -- 493 = 0755 octal
        if mkdir_err and not mkdir_err:match('EEXIST') then
          callback(false, 'Failed to create directory: ' .. mkdir_err)
          return
        end
        callback(true, nil)
      end)
    end)
  end)
end

return M
