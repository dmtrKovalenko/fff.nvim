--- Colorscheme source adapter for the shared fff-plus picker.

local matcher = require('fff_plus.matcher')
local picker = require('fff_plus.picker')

local M = {}

function M.get_colorschemes()
  local colorschemes = {}
  local seen = {}
  local patterns = {
    { vim.o.runtimepath, 'colors/*.vim' },
    { vim.o.runtimepath, 'colors/*.lua' },
    { vim.o.packpath, 'pack/*/opt/*/colors/*.vim' },
    { vim.o.packpath, 'pack/*/opt/*/colors/*.lua' },
    { vim.o.packpath, 'pack/*/start/*/colors/*.vim' },
    { vim.o.packpath, 'pack/*/start/*/colors/*.lua' },
  }

  for _, pattern in ipairs(patterns) do
    for _, file in ipairs(vim.fn.globpath(pattern[1], pattern[2], false, true)) do
      local name = vim.fn.fnamemodify(file, ':t:r')
      if name ~= '' and not seen[name] then
        seen[name] = true
        table.insert(colorschemes, name)
      end
    end
  end

  table.sort(colorschemes)
  local current = vim.g.colors_name
  if current then
    for index, name in ipairs(colorschemes) do
      if name == current then
        table.remove(colorschemes, index)
        break
      end
    end
    table.insert(colorschemes, 1, current)
  end
  return colorschemes
end

function M.format_colorscheme(name, index)
  return {
    name = name,
    path = name,
    relative_path = name,
    display_name = name,
    current = vim.g.colors_name == name,
    is_dir = false,
    idx = index,
  }
end

function M.get_colorscheme_items()
  local items = {}
  for index, name in ipairs(M.get_colorschemes()) do
    table.insert(items, M.format_colorscheme(name, index))
  end
  return items
end

function M.filter_colorschemes(items, query)
  return matcher.filter(items, query, function(item) return item.name or '' end)
end

function M.create(opts)
  opts = vim.tbl_deep_extend('force', {
    title = 'Colors',
    prompt = 'Colors> ',
    preview = { enabled = false },
    keymaps = {
      select_split = false,
      select_vsplit = false,
      select_tab = false,
    },
  }, opts or {})

  local original = vim.g.colors_name
  return picker.create({
    name = 'colors',
    title = 'Colors',
    resume = function(resume_opts) return M.open(resume_opts) end,
    items = function() return M.get_colorscheme_items() end,
    key = function(item) return item.name end,
    text = function(item) return item.name end,
    format = function(item) return { text = (item.current and '* ' or '  ') .. item.name, match_offset = 2 } end,
    on_change = function(_, item)
      if item then pcall(vim.cmd.colorscheme, item.name) end
    end,
    confirm = function(_, item)
      return function() vim.cmd.colorscheme(item.name) end
    end,
    on_close = function(_, confirmed)
      if not confirmed and original then pcall(vim.cmd.colorscheme, original) end
    end,
  }, opts)
end

function M.open(opts)
  if M.state and M.state.active then return M.state end
  M.state = M.create(opts)
  return M.state:open()
end

return M
