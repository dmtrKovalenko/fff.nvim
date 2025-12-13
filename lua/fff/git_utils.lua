local M = {}

-- git signs like borders
M.border_chars = {
  untracked = '┆', -- Dotted vertical line
  ignored = '┆', -- Dotted vertical line
  unknown = '┆',
  modified = '┃', -- Vertical line
  deleted = '▁', -- Bottom horizontal line
  renamed = '┃', -- Vertical line
  staged_new = '┃', -- Vertical line
  staged_modified = '┃', -- Vertical line
  staged_deleted = '▁', -- Bottom horizontal line
  clean = '',
  clear = '',
}

-- Cache for config-based highlight mappings
local highlights_cache = nil
local border_highlights_cache = nil
local border_highlights_selected_cache = nil

--- Build and cache highlight mappings from config
local function ensure_cache()
  if highlights_cache then return end

  local config = require('fff.conf').get()

  highlights_cache = {
    untracked = config.hl.git_untracked,
    modified = config.hl.git_modified,
    deleted = config.hl.git_deleted,
    renamed = config.hl.git_renamed,
    staged_new = config.hl.git_staged,
    staged_modified = config.hl.git_staged,
    staged_deleted = config.hl.git_staged,
    ignored = config.hl.git_ignored,
    clean = '',
    clear = '',
    unknown = config.hl.git_untracked,
  }

  border_highlights_cache = {
    untracked = config.hl.git_sign_untracked,
    modified = config.hl.git_sign_modified,
    deleted = config.hl.git_sign_deleted,
    renamed = config.hl.git_sign_renamed,
    staged_new = config.hl.git_sign_staged,
    staged_modified = config.hl.git_sign_staged,
    staged_deleted = config.hl.git_sign_staged,
    ignored = config.hl.git_sign_ignored,
    clean = '',
    clear = '',
    unknown = config.hl.git_sign_untracked,
  }

  border_highlights_selected_cache = {
    untracked = config.hl.git_sign_untracked_selected,
    modified = config.hl.git_sign_modified_selected,
    deleted = config.hl.git_sign_deleted_selected,
    renamed = config.hl.git_sign_renamed_selected,
    staged_new = config.hl.git_sign_staged_selected,
    staged_modified = config.hl.git_sign_staged_selected,
    staged_deleted = config.hl.git_sign_staged_selected,
    ignored = config.hl.git_sign_ignored_selected,
    clean = '',
    clear = '',
    unknown = config.hl.git_sign_untracked_selected,
  }
end

--- Get highlight group for git status text
--- @param git_status string Git status
--- @return string Highlight group name
function M.get_highlight(git_status)
  ensure_cache()
  return highlights_cache[git_status] or ''
end

--- Get border highlight group for git status
--- @param git_status string Git status
--- @return string Highlight group name
function M.get_border_highlight(git_status)
  ensure_cache()
  return border_highlights_cache[git_status] or ''
end

--- Get selected border highlight group for git status
--- @param git_status string Git status
--- @return string Highlight group name
function M.get_border_highlight_selected(git_status)
  ensure_cache()
  return border_highlights_selected_cache[git_status] or ''
end

function M.get_border_char(git_status) return M.border_chars[git_status] or '' end

function M.should_show_border(git_status)
  return git_status == 'untracked'
    or git_status == 'modified'
    or git_status == 'staged_new'
    or git_status == 'staged_modified'
    or git_status == 'deleted'
    or git_status == 'staged_deleted'
    or git_status == 'renamed'
end

function M.setup_highlights()
  vim.cmd([[
    " Symbol highlights
    highlight default FFFGitStaged guifg=#10B981 ctermfg=2
    highlight default FFFGitModified guifg=#F59E0B ctermfg=3  
    highlight default FFFGitDeleted guifg=#EF4444 ctermfg=1
    highlight default FFFGitRenamed guifg=#8B5CF6 ctermfg=5
    highlight default FFFGitUntracked guifg=#10B981 ctermfg=2
    highlight default FFFGitIgnored guifg=#4B5563 ctermfg=8
    
    " Thin border highlights 
    highlight default FFFGitSignStaged guifg=#10B981 ctermfg=2
    highlight default FFFGitSignModified guifg=#F59E0B ctermfg=3  
    highlight default FFFGitSignDeleted guifg=#EF4444 ctermfg=1
    highlight default FFFGitSignRenamed guifg=#8B5CF6 ctermfg=5
    highlight default FFFGitSignUntracked guifg=#10B981 ctermfg=2
    highlight default FFFGitSignIgnored guifg=#4B5563 ctermfg=8
    
    " Fallback to GitSigns highlights if they exist
    highlight default link FFFGitSignStaged GitSignsAdd
    highlight default link FFFGitSignModified GitSignsChange
    highlight default link FFFGitSignDeleted GitSignsDelete
    highlight default link FFFGitSignUntracked GitSignsAdd
  ]])

  -- Highlighes for git signs both for selected and normal states
  local git_highlights = {
    { 'FFFGitSignStaged', 'FFFGitSignStagedSelected', '#10B981', 2 },
    { 'FFFGitSignModified', 'FFFGitSignModifiedSelected', '#F59E0B', 3 },
    { 'FFFGitSignDeleted', 'FFFGitSignDeletedSelected', '#EF4444', 1 },
    { 'FFFGitSignRenamed', 'FFFGitSignRenamedSelected', '#8B5CF6', 5 },
    { 'FFFGitSignUntracked', 'FFFGitSignUntrackedSelected', '#10B981', 2 },
    { 'FFFGitSignIgnored', 'FFFGitSignIgnoredSelected', '#4B5563', 8 },
  }

  for _, hl in ipairs(git_highlights) do
    local _, selected_hl, gui_fg, cterm_fg = hl[1], hl[2], hl[3], hl[4]

    local visual_bg_gui = vim.fn.synIDattr(vim.fn.synIDtrans(vim.fn.hlID('Visual')), 'bg', 'gui')
    local visual_bg_cterm = vim.fn.synIDattr(vim.fn.synIDtrans(vim.fn.hlID('Visual')), 'bg', 'cterm')

    local gui_bg = visual_bg_gui ~= '' and visual_bg_gui or 'NONE'
    local cterm_bg = visual_bg_cterm ~= '' and visual_bg_cterm or 'NONE'

    vim.cmd(
      string.format(
        'highlight default %s guifg=%s guibg=%s ctermfg=%d ctermbg=%s',
        selected_hl,
        gui_fg,
        gui_bg,
        cterm_fg,
        cterm_bg
      )
    )
  end
end

return M
