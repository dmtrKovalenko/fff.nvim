local renderer = require('fff.picker_ui.file_renderer')

local icons = require('fff.file_picker.icons')
local highlights = require('fff.highlights')
local file_picker = require('fff.file_picker')
local rust = require('fff.rust')

local path_separator = package.config:sub(1, 1)
local directory = table.concat({ 'src', 'components' }, path_separator)
local relative_path = directory .. path_separator .. 'main.lua'

local original_get_icon = icons.get_icon
local original_get_git_text_highlight = highlights.get_git_text_highlight
local original_should_show_git_border = highlights.should_show_git_border
local original_get_file_score = file_picker.get_file_score

local function make_context()
  return {
    cursor = 0,
    query = 'sm',
    max_path_width = 80,
    win_width = 80,
    debug_enabled = false,
    selected_files = {},
    config = {
      file_picker = { display_relative_path = true, fuzzy_query_highlighting = true },
      git = { status_text_color = true },
      hl = { directory_path = 'Comment', matched = 'Search' },
    },
    format_file_display = function() return 'main.lua', directory end,
  }
end

local function highlight_ranges(buf, ns, group)
  local ranges = {}
  for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(buf, ns, 0, -1, { details = true })) do
    local details = mark[4]
    if details.hl_group == group then ranges[#ranges + 1] = { mark[3], details.end_col } end
  end
  table.sort(ranges, function(a, b) return a[1] < b[1] end)
  return ranges
end

describe('file renderer relative path display', function()
  before_each(function()
    icons.get_icon = function() return 'I', 'Icon' end
    highlights.get_git_text_highlight = function() return 'GitText' end
    highlights.should_show_git_border = function() return false end
    file_picker.get_file_score = function() return nil end
  end)

  after_each(function()
    icons.get_icon = original_get_icon
    highlights.get_git_text_highlight = original_get_git_text_highlight
    highlights.should_show_git_border = original_should_show_git_border
    file_picker.get_file_score = original_get_file_score
  end)

  it('renders a natural path and highlights only its filename for git status', function()
    local item = {
      name = 'main.lua',
      relative_path = relative_path,
      git_status = 'modified',
      match_ranges = { { 0, 1 }, { 15, 16 } },
    }
    local ctx = make_context()
    local line = renderer.render_line(item, ctx)[1]
    assert.are.equal('I ' .. relative_path, vim.trim(line))

    local buf = vim.api.nvim_create_buf(false, true)
    local ns = vim.api.nvim_create_namespace('fff-file-renderer-test')
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { line })
    renderer.apply_highlights(item, ctx, 1, buf, ns, 1, line)

    local filename_start = assert(line:find('main.lua', 1, true)) - 1
    assert.are.same({ { filename_start, filename_start + #'main.lua' } }, highlight_ranges(buf, ns, 'GitText'))
    assert.are.same({ { 2, 3 }, { filename_start, filename_start + 1 } }, highlight_ranges(buf, ns, 'Search'))
    vim.api.nvim_buf_delete(buf, { force = true })
  end)

  it('shortens a long directory while keeping the filename visible', function()
    local filename = 'main.lua'
    local long_directory = table.concat({ 'very', 'long', 'nested', 'directory' }, path_separator)
    local item = { name = filename, relative_path = long_directory .. path_separator .. filename }
    local ctx = make_context()
    ctx.max_path_width = 22
    ctx.win_width = 22
    ctx.format_file_display = function(_, available_width)
      local directory_width = math.max(available_width - vim.fn.strdisplaywidth(filename) - 1, 0)
      return filename, rust.shorten_path(long_directory, directory_width, 'middle')
    end

    local line = vim.trim(renderer.render_line(item, ctx)[1])
    assert.is_true(line:sub(-#filename) == filename)
    assert.is_nil(line:find(long_directory, 1, true))
    assert.is_true(vim.fn.strdisplaywidth(line) <= ctx.max_path_width)
  end)
end)
