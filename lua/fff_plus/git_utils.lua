local highlights = require('fff.highlights')

local M = {}

function M.get_border_char(git_status) return highlights.get_git_border_char(git_status) end

function M.get_border_highlight(git_status) return highlights.get_git_border_highlight(git_status) end

function M.get_border_highlight_selected(git_status) return highlights.get_git_border_highlight_selected(git_status) end

function M.should_show_border(git_status) return highlights.should_show_git_border(git_status) end

return M
