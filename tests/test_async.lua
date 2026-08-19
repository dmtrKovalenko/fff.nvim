package.loaded['fff.conf'] = {
  get = function()
    return {
      prompt = '> ',
      layout = {},
      preview = { enabled = false },
      keymaps = {},
      hl = {},
    }
  end,
}

local process = require('fff_plus.process')

local captured
local completed
local handle = process.run({ 'git', 'status', '--porcelain' }, {
  cwd = '/repo',
  system = function(command, opts, callback)
    captured = { command = command, opts = opts }
    callback({ code = 0, stdout = 'ok', stderr = '' })
    return { kill = function() end }
  end,
}, function(result) completed = result end)

assert(captured.command[1] == 'git' and captured.command[3] == '--porcelain', 'process should preserve argv')
assert(captured.opts.cwd == '/repo', 'process should pass cwd separately')
assert(completed.ok and completed.stdout == 'ok', 'process should normalize successful results')
assert(type(handle.kill) == 'function', 'process should return the cancellable handle')

local picker = require('fff_plus.picker')
local callbacks = {}
local cancelled = {}
local instance = picker.create({
  name = 'async-memory',
  items = function(_, done)
    table.insert(callbacks, done)
    local index = #callbacks
    return {
      cancel = function() cancelled[index] = true end,
    }
  end,
}, {})

instance:refresh()
instance:refresh()
assert(cancelled[1] == true, 'refresh should cancel the previous source job')

callbacks[1]({ { text = 'stale' } })
callbacks[2]({ { text = 'current' } })
assert(instance:current().text == 'current', 'stale source callbacks should not replace newer results')

print('Async process and picker source tests passed')
