---@diagnostic disable: undefined-field, missing-fields
local fff_rust = require('fff.rust')

describe('clear_cache', function()
  local test_dir
  local tmp_frecency_path
  local tmp_history_path

  before_each(function()
    test_dir = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':h:h')
    if vim.fn.isdirectory(test_dir) ~= 1 then test_dir = vim.fn.getcwd() end

    tmp_frecency_path = vim.fn.tempname() .. '_fff_test_frecency'
    tmp_history_path = vim.fn.tempname() .. '_fff_test_history'

    vim.g.fff = {
      frecency = { enabled = true, db_path = tmp_frecency_path },
      history = { enabled = true, db_path = tmp_history_path },
    }
    package.loaded['fff.conf'] = nil
    package.loaded['fff.main'] = nil
  end)

  after_each(function()
    pcall(fff_rust.stop_background_monitor)
    pcall(fff_rust.cleanup_file_picker)
    pcall(fff_rust.destroy_frecency_db)
    pcall(fff_rust.destroy_query_db)
    vim.fn.delete(tmp_frecency_path, 'rf')
    vim.fn.delete(tmp_history_path, 'rf')
    vim.g.fff = nil
    package.loaded['fff.conf'] = nil
    package.loaded['fff.main'] = nil
  end)

  it('deletes on-disk database directories when clearing all', function()
    -- Initialize databases at temporary paths
    local ok = fff_rust.init_db(tmp_frecency_path, tmp_history_path, true)
    assert.is_true(ok)

    -- LMDB creates the directory on init
    assert.are.equal(1, vim.fn.isdirectory(tmp_frecency_path), 'frecency db dir should exist after init')
    assert.are.equal(1, vim.fn.isdirectory(tmp_history_path), 'history db dir should exist after init')

    local main = require('fff.main')
    local result = main.clear_cache('all')
    assert.is_true(result)

    assert.are.equal(0, vim.fn.isdirectory(tmp_frecency_path), 'frecency db dir should be removed after clear')
    assert.are.equal(0, vim.fn.isdirectory(tmp_history_path), 'history db dir should be removed after clear')
  end)

  it('deletes only frecency databases when scope is frecency', function()
    local ok = fff_rust.init_db(tmp_frecency_path, tmp_history_path, true)
    assert.is_true(ok)
    ok = fff_rust.init_file_picker(test_dir)
    assert.is_true(ok)
    fff_rust.wait_for_initial_scan(10000)

    local main = require('fff.main')
    local result = main.clear_cache('frecency')
    assert.is_true(result)

    assert.are.equal(0, vim.fn.isdirectory(tmp_frecency_path), 'frecency db dir should be removed')
    assert.are.equal(0, vim.fn.isdirectory(tmp_history_path), 'history db dir should be removed')

    local progress = fff_rust.get_scan_progress()
    assert.is_not_nil(progress)
    assert.is_true(progress.scanned_files_count > 0, 'file picker should still have scanned files')
  end)

  it('cleans file picker but keeps databases when scope is files', function()
    local ok = fff_rust.init_db(tmp_frecency_path, tmp_history_path, true)
    assert.is_true(ok)
    ok = fff_rust.init_file_picker(test_dir)
    assert.is_true(ok)
    fff_rust.wait_for_initial_scan(10000)

    local main = require('fff.main')
    local result = main.clear_cache('files')
    assert.is_true(result)

    assert.are.equal(1, vim.fn.isdirectory(tmp_frecency_path), 'frecency db dir should still exist')
    assert.are.equal(1, vim.fn.isdirectory(tmp_history_path), 'history db dir should still exist')
  end)
end)
