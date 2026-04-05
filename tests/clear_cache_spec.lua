---@diagnostic disable: undefined-global

describe('clear_cache', function()
  local main
  local old_notify
  local notifications
  local calls

  before_each(function()
    package.loaded['fff.main'] = nil
    package.loaded['fff.fuzzy'] = nil
    notifications = {}
    calls = {}
    old_notify = vim.notify
    vim.notify = function(msg, level)
      table.insert(notifications, { msg = msg, level = level })
    end
    package.loaded['fff.fuzzy'] = {
      cleanup_file_picker = function() table.insert(calls, 'cleanup_file_picker') end,
      destroy_db = function() table.insert(calls, 'destroy_db') end,
      destroy_query_db = function() table.insert(calls, 'destroy_query_db') end,
    }
    main = require('fff.main')
  end)

  after_each(function()
    vim.notify = old_notify
    package.loaded['fff.main'] = nil
    package.loaded['fff.fuzzy'] = nil
  end)

  it('clears all caches by default', function()
    assert.is_true(main.clear_cache())
    assert.are.same({ 'cleanup_file_picker', 'destroy_db', 'destroy_query_db' }, calls)
  end)

  it('clears only frecency cache when requested', function()
    assert.is_true(main.clear_cache('frecency'))
    assert.are.same({ 'destroy_query_db' }, calls)
  end)

  it('clears only file cache when requested', function()
    assert.is_true(main.clear_cache('files'))
    assert.are.same({ 'cleanup_file_picker', 'destroy_db' }, calls)
  end)
end)
