local M = {}

function M.run(command, opts, done)
  vim.validate({
    command = { command, 'table' },
    opts = { opts, 'table', true },
    done = { done, 'function' },
  })

  opts = opts or {}
  local system = opts.system or vim.system
  local system_opts = {
    cwd = opts.cwd,
    env = opts.env,
    stdin = opts.stdin,
    text = opts.text ~= false,
  }

  return system(command, system_opts, function(result)
    local normalized = {
      ok = result.code == 0,
      code = result.code,
      signal = result.signal,
      stdout = result.stdout or '',
      stderr = result.stderr or '',
    }
    if vim.in_fast_event() then
      vim.schedule(function() done(normalized) end)
    else
      done(normalized)
    end
  end)
end

return M
