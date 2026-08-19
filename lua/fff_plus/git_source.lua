local process = require('fff_plus.process')

local M = {}

local function split_nul(output)
  local values = {}
  local start = 1

  while start <= #output do
    local finish = output:find('\0', start, true)
    if not finish then break end
    table.insert(values, output:sub(start, finish - 1))
    start = finish + 1
  end

  return values
end

local function classify(index_status, worktree_status)
  if index_status == '?' and worktree_status == '?' then return 'untracked' end
  if index_status == '!' and worktree_status == '!' then return 'ignored' end
  if index_status == 'R' or worktree_status == 'R' or index_status == 'C' or worktree_status == 'C' then
    return 'renamed'
  end
  if index_status == 'A' then return 'staged_new' end
  if index_status == 'M' then return 'staged_modified' end
  if index_status == 'D' then return 'staged_deleted' end
  if worktree_status == 'M' then return 'modified' end
  if worktree_status == 'D' then return 'deleted' end
  if worktree_status == 'A' then return 'untracked' end
  return 'unknown'
end

function M.parse_tracked(output) return split_nul(output or '') end

function M.parse_status(output)
  local fields = split_nul(output or '')
  local entries = {}
  local index = 1

  while index <= #fields do
    local record = fields[index]
    if #record >= 3 then
      local index_status = record:sub(1, 1)
      local worktree_status = record:sub(2, 2)
      local renamed = index_status == 'R' or worktree_status == 'R' or index_status == 'C' or worktree_status == 'C'
      local entry = {
        relative_path = record:sub(4),
        git_status = classify(index_status, worktree_status),
      }

      if renamed then
        entry.old_path = fields[index + 1]
        index = index + 1
      end

      table.insert(entries, entry)
    end
    index = index + 1
  end

  return entries
end

function M.run(command, opts, done) return process.run(command, opts, done) end

local function transform(command, cwd, parse, fallback, done)
  return M.run(command, { cwd = cwd }, function(result)
    if not result.ok then
      done(fallback, result)
      return
    end
    done(parse(result.stdout), result)
  end)
end

function M.root(cwd, done)
  return transform(
    { 'git', 'rev-parse', '--show-toplevel' },
    cwd,
    function(output) return vim.trim(output) end,
    nil,
    done
  )
end

function M.tracked(git_root, done) return transform({ 'git', 'ls-files', '-z' }, git_root, M.parse_tracked, {}, done) end

function M.status(git_root, done)
  return transform(
    { 'git', 'status', '--porcelain=v1', '-z', '--untracked-files=all' },
    git_root,
    M.parse_status,
    {},
    done
  )
end

function M.diff(git_root, relative_path, done)
  return transform(
    { 'git', 'diff', '--no-ext-diff', 'HEAD', '--', relative_path },
    git_root,
    function(output) return output ~= '' and output or nil end,
    nil,
    done
  )
end

local function mutate(git_root, command, paths, done)
  local argv = { 'git' }
  vim.list_extend(argv, command)
  table.insert(argv, '--')
  vim.list_extend(argv, paths)
  return M.run(argv, { cwd = git_root }, function(result) done(result.ok, result) end)
end

function M.stage(git_root, paths, done) return mutate(git_root, { 'add' }, paths, done) end

function M.unstage(git_root, paths, done) return mutate(git_root, { 'restore', '--staged' }, paths, done) end

function M.restore(git_root, paths, done) return mutate(git_root, { 'restore', '--worktree' }, paths, done) end

return M
