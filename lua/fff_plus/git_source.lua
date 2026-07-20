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

function M.run(git_root, args)
  local command = { 'git', '-C', vim.fn.shellescape(git_root) }
  for _, argument in ipairs(args) do
    table.insert(command, vim.fn.shellescape(argument))
  end

  local handle = io.popen(table.concat(command, ' ') .. ' 2>/dev/null')
  if not handle then return nil end
  local output = handle:read('*a')
  local ok = handle:close()
  if not ok then return nil end
  return output
end

function M.tracked(git_root)
  local output = M.run(git_root, { 'ls-files', '-z' })
  if not output then return {} end
  return M.parse_tracked(output)
end

function M.status(git_root)
  local output = M.run(git_root, { 'status', '--porcelain=v1', '-z', '--untracked-files=all' })
  if not output then return {} end
  return M.parse_status(output)
end

function M.diff(git_root, relative_path)
  local output = M.run(git_root, { 'diff', '--no-ext-diff', 'HEAD', '--', relative_path })
  if not output or output == '' then return nil end
  return output
end

return M
