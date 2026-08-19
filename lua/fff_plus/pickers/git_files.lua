--- Git tracked-files and status adapters for the shared fff-plus picker.

local preview = require('fff.file_picker.preview')
local git_source = require('fff_plus.git_source')
local git_utils = require('fff_plus.git_utils')
local matcher = require('fff_plus.matcher')
local picker = require('fff_plus.picker')
local selection = require('fff_plus.selection')

local M = {}

local function format_git_file(git_root, relative_path, git_status, old_path)
  local name = vim.fn.fnamemodify(relative_path, ':t')
  return {
    name = name,
    path = git_root .. '/' .. relative_path,
    relative_path = relative_path,
    old_path = old_path,
    directory = vim.fn.fnamemodify(relative_path, ':h'),
    extension = vim.fn.fnamemodify(name, ':e'),
    git_status = git_status,
    is_dir = false,
  }
end

function M.get_git_root(cwd, done) return git_source.root(cwd or vim.fn.getcwd(), done) end

function M.get_git_status_files(git_root, done)
  return git_source.status(git_root, function(entries, result)
    local files = {}
    for _, entry in ipairs(entries) do
      table.insert(files, format_git_file(git_root, entry.relative_path, entry.git_status, entry.old_path))
    end
    done(files, result)
  end)
end

function M.get_tracked_files(git_root, done)
  return git_source.tracked(git_root, function(paths, result)
    local files = {}
    for _, relative_path in ipairs(paths) do
      table.insert(files, format_git_file(git_root, relative_path, 'clean'))
    end
    done(files, result)
  end)
end

function M.filter_files(items, query)
  return matcher.filter(items, query, function(item) return item.relative_path or '' end)
end

local function load_items(instance, done)
  local job = {}
  local root_finished = false

  function job:cancel()
    if not self.active then return end
    local cancel = self.active.cancel or self.active.kill
    if cancel then pcall(cancel, self.active, 15) end
    self.active = nil
  end

  local root_job = M.get_git_root(instance.opts.cwd, function(git_root, result)
    root_finished = true
    if not git_root or git_root == '' then
      instance.git_root = nil
      instance.source_error = result and result.stderr or 'Not in a Git repository'
      done({})
      if instance.active then vim.notify('Not in a Git repository', vim.log.levels.WARN) end
      return
    end

    instance.git_root = git_root
    local load = instance.source == 'tracked' and M.get_tracked_files or M.get_git_status_files
    job.active = load(git_root, function(items, source_result)
      instance.source_error = source_result and not source_result.ok and source_result.stderr or nil
      done(items)
    end)
  end)
  if not root_finished then job.active = root_job end
  return job
end

local function format_item(item)
  local sign
  if git_utils.should_show_border(item.git_status) then
    sign = {
      text = git_utils.get_border_char(item.git_status),
      hl = git_utils.get_border_highlight(item.git_status),
    }
  end
  return { text = '  ' .. item.relative_path, sign = sign, match_offset = 2 }
end

local function preview_file(instance, item)
  if preview.setup then preview.setup(instance.config.preview) end
  if preview.set_preview_window then preview.set_preview_window(instance.preview_win) end
  if preview.preview then preview.preview(item.path, instance.preview_buf) end
  if instance.preview_win and vim.api.nvim_win_is_valid(instance.preview_win) then
    vim.api.nvim_win_set_config(instance.preview_win, { title = ' ' .. item.relative_path .. ' ', title_pos = 'left' })
  end
end

local function preview_item(instance, item, done)
  if instance.source ~= 'status' or item.git_status == 'untracked' then return preview_file(instance, item) end
  return git_source.diff(instance.git_root, item.relative_path, function(diff)
    if not diff then
      preview_file(instance, item)
      return
    end
    done({
      title = item.relative_path,
      lines = vim.split(diff, '\n', { plain = true, trimempty = true }),
      filetype = 'diff',
    })
  end)
end

local function chosen(instance) return instance:selected({ fallback = true }) end

local function send_list(instance, location)
  local entries = {}
  for _, item in ipairs(chosen(instance)) do
    table.insert(entries, { filename = item.path, lnum = 1, col = 1, text = item.relative_path })
  end
  if #entries == 0 then return end

  instance:close(true)
  if location then
    vim.fn.setloclist(0, {}, ' ', { title = 'FFF+ Git Files', items = entries })
    vim.cmd('lopen')
  else
    vim.fn.setqflist({}, ' ', { title = 'FFF+ Git Files', items = entries })
    vim.cmd('copen')
  end
end

local function paste(instance)
  local items = chosen(instance)
  local origin = instance.origin_win
  instance:close(true)
  if origin and vim.api.nvim_win_is_valid(origin) then vim.api.nvim_set_current_win(origin) end
  selection.put(items, function(item) return item.relative_path end)
end

local function mutate(instance, operation)
  local items = chosen(instance)
  if #items == 0 or not instance.git_root then return end
  local paths = vim.tbl_map(function(item) return item.relative_path end, items)
  instance.mutation_job = git_source[operation](instance.git_root, paths, function(ok, result)
    instance.mutation_job = nil
    if not ok then
      local message = result and vim.trim(result.stderr or '') or ''
      vim.notify(message ~= '' and message or ('Git ' .. operation .. ' failed'), vim.log.levels.ERROR)
      return
    end
    instance.selected_keys = {}
    instance:refresh()
  end)
end

local function restore(instance)
  local items = chosen(instance)
  if #items == 0 then return end
  local confirm = instance.opts.confirm_restore or vim.fn.confirm
  local answer = confirm(
    string.format('Discard worktree changes in %d file%s?', #items, #items == 1 and '' or 's'),
    '&Discard\n&Cancel',
    2
  )
  if answer == 1 then mutate(instance, 'restore') end
end

function M.create(opts)
  opts = vim.tbl_deep_extend('force', {
    prompt = '🦆 ',
    keymaps = { paste = '<A-CR>' },
  }, opts or {})
  local source = opts.source or 'status'
  if opts.title == nil then opts.title = source == 'tracked' and 'Git Files' or 'Git Status' end

  local instance = picker.create({
    name = 'git_files',
    title = opts.title,
    resume = function(resume_opts) return M.open(resume_opts) end,
    items = load_items,
    key = function(item) return item.relative_path end,
    text = function(item) return item.relative_path end,
    format = format_item,
    preview = preview_item,
    confirm = function(_, item, action)
      return function()
        local command = action == 'split' and 'split'
          or (action == 'vsplit' and 'vsplit' or (action == 'tab' and 'tabnew' or 'edit'))
        vim.cmd(command .. ' ' .. vim.fn.fnameescape(item.path))
      end
    end,
    actions = {
      qflist = function(ctx) send_list(ctx, false) end,
      loclist = function(ctx) send_list(ctx, true) end,
      paste = paste,
      stage = function(ctx) mutate(ctx, 'stage') end,
      unstage = function(ctx) mutate(ctx, 'unstage') end,
      restore = restore,
      refresh = function(ctx) ctx:refresh() end,
      preview_scroll_up = function(_, _)
        if preview.scroll then preview.scroll(-10) end
      end,
      preview_scroll_down = function(_, _)
        if preview.scroll then preview.scroll(10) end
      end,
    },
    keymaps = {
      stage = '<A-s>',
      unstage = '<A-u>',
      restore = '<A-r>',
      refresh = '<F5>',
    },
    on_close = function(ctx)
      if ctx.mutation_job then
        local cancel = ctx.mutation_job.cancel or ctx.mutation_job.kill
        if cancel then pcall(cancel, ctx.mutation_job, 15) end
      end
      if ctx.preview_buf and preview.clear_buffer then preview.clear_buffer(ctx.preview_buf) end
    end,
  }, opts)
  instance.source = source
  instance.origin_win = vim.api.nvim_get_current_win()
  return instance
end

function M.open(opts)
  if M.state and M.state.active then return M.state end
  M.state = M.create(opts)
  return M.state:open()
end

function M.get_git_diff(item, done)
  if not M.state or M.state.source ~= 'status' or item.git_status == 'untracked' then
    if done then done(nil) end
    return nil
  end
  return git_source.diff(M.state.git_root, item.relative_path, done)
end

function M.filter_results()
  if not M.state then return end
  M.state:apply_query()
  M.state:changed()
end

function M.toggle_selection() return M.state and M.state:toggle_selection() end

function M.send_to_quickfix() return M.state and M.state:action('qflist') end

function M.paste_selection() return M.state and M.state:action('paste') end

function M.select(action) return M.state and M.state:confirm(action) end

function M.close() return M.state and M.state:close(false) end

return M
