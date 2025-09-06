local M = {}

local conf = require('fff.conf')
local file_picker = require('fff.file_picker')

---@class FFFSnacksState
---@field current_file_cache? string
---@field config table FFF config
M.state = { config = {} }

local staged_status = {
  staged_new = true,
  staged_modified = true,
  staged_deleted = true,
  renamed = true,
}

local status_map = {
  untracked = 'untracked',
  modified = 'modified',
  deleted = 'deleted',
  renamed = 'renamed',
  staged_new = 'added',
  staged_modified = 'modified',
  staged_deleted = 'deleted',
  ignored = 'ignored',
  -- clean = "",
  -- clear = "",
  unknown = 'untracked',
}

--- tweaked version of `Snacks.picker.format.file_git_status`
--- @type snacks.picker.format
local function format_file_git_status(item, picker)
  local ret = {} ---@type snacks.picker.Highlight[]
  local status = item.status

  local hl = 'SnacksPickerGitStatus'
  if status.unmerged then
    hl = 'SnacksPickerGitStatusUnmerged'
  elseif status.staged then
    hl = 'SnacksPickerGitStatusStaged'
  else
    hl = 'SnacksPickerGitStatus' .. status.status:sub(1, 1):upper() .. status.status:sub(2)
  end

  local icon = picker.opts.icons.git[status.status]
  if status.staged then icon = picker.opts.icons.git.staged end

  local text_icon = status.status:sub(1, 1):upper()
  text_icon = status.status == 'untracked' and '?' or status.status == 'ignored' and '!' or text_icon

  ret[#ret + 1] = { icon, hl }
  ret[#ret + 1] = { ' ', virtual = true }

  ret[#ret + 1] = {
    col = 0,
    virt_text = { { text_icon, hl }, { ' ' } },
    virt_text_pos = 'right_align',
    hl_mode = 'combine',
  }
  return ret
end

---@type snacks.picker.Config
M.source = {
  title = 'FFFiles',
  finder = function(opts, ctx)
    -- initialization code from require('fff.picker_ui').open
    -- on_show does not seem to be called before finder
    if not M.state.current_file_cache then
      local current_buf = vim.api.nvim_get_current_buf()
      if current_buf and vim.api.nvim_buf_is_valid(current_buf) then
        local current_file = vim.api.nvim_buf_get_name(current_buf)
        if current_file ~= '' and vim.fn.filereadable(current_file) == 1 then
          M.state.current_file_cache = current_file
        else
          M.state.current_file_cache = nil
        end
      end
    end
    if not file_picker.is_initialized() then
      if not file_picker.setup() then
        vim.notify('Failed to initialize file picker', vim.log.levels.ERROR)
        return {}
      end
    end
    local config = conf.get()
    M.state.config = vim.tbl_deep_extend('force', config or {}, opts or {})

    local fff_result = file_picker.search_files(
      ctx.filter.search,
      opts.limit or M.state.config.max_results,
      M.state.config.max_threads,
      M.state.current_file_cache,
      false
    )

    ---@type snacks.picker.finder.Item[]
    local items = {}
    for _, fff_item in ipairs(fff_result) do
      ---@type snacks.picker.finder.Item
      local item = {
        text = fff_item.name,
        file = fff_item.path,
        score = fff_item.total_frecency_score,
        -- HACK: in original snacks implementation status is a string of
        -- `git status --porcelain` output
        status = status_map[fff_item.git_status] and {
          status = status_map[fff_item.git_status],
          staged = staged_status[fff_item.git_status] or false,
          unmerged = fff_item.git_status == 'unmerged',
        },
      }
      items[#items + 1] = item
    end

    return items
  end,
  format = function(item, picker)
    ---@type snacks.picker.Highlight[]
    local ret = {}

    if item.label then
      ret[#ret + 1] = { item.label, 'SnacksPickerLabel' }
      ret[#ret + 1] = { ' ', virtual = true }
    end

    if item.status then
      vim.list_extend(ret, format_file_git_status(item, picker))
    else
      ret[#ret + 1] = { '  ', virtual = true }
    end

    vim.list_extend(ret, require('snacks').picker.format.filename(item, picker))

    if item.line then
      require('snacks').picker.highlight.format(item, item.line, ret)
      table.insert(ret, { ' ' })
    end
    return ret
  end,
  on_close = function() M.state.current_file_cache = nil end,
  formatters = {
    file = {
      filename_first = true,
    },
  },
  live = true,
}

return M
