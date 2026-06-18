local M = {}

-- Git sign border characters per status
M.git_border_chars = {
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

local git_text_highlights_cache = nil
local git_border_highlights_cache = nil
local git_border_highlights_selected_cache = nil
-- Cache for cursor-blended git border highlights, keyed by git_status.
-- Cleared on setup() since it depends on user's Cursor/Visual colours.
local git_cursor_border_cache = {}

local function ensure_git_cache()
  if git_text_highlights_cache then return end

  local config = require('fff.conf').get()

  git_text_highlights_cache = {
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

  git_border_highlights_cache = {
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

  git_border_highlights_selected_cache = {
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

--- Get git sign border character for a status
--- @param git_status string Git status
--- @return string Border character
function M.get_git_border_char(git_status) return M.git_border_chars[git_status] or '' end

--- Get highlight group for git status text (filename)
--- @param git_status string Git status
--- @return string Highlight group name
function M.get_git_text_highlight(git_status)
  ensure_git_cache()
  return git_text_highlights_cache and git_text_highlights_cache[git_status] or ''
end

--- Get sign-column border highlight group for git status
--- @param git_status string Git status
--- @return string Highlight group name
function M.get_git_border_highlight(git_status)
  ensure_git_cache()
  return git_border_highlights_cache and git_border_highlights_cache[git_status] or ''
end

--- Get sign-column border highlight group for git status when row is selected
--- @param git_status string Git status
--- @return string Highlight group name
function M.get_git_border_highlight_selected(git_status)
  ensure_git_cache()
  return git_border_highlights_selected_cache and git_border_highlights_selected_cache[git_status] or ''
end

--- Whether a git status warrants a sign-column border at all
--- @param git_status string Git status
--- @return boolean
function M.should_show_git_border(git_status)
  return git_status == 'untracked'
    or git_status == 'modified'
    or git_status == 'staged_new'
    or git_status == 'staged_modified'
    or git_status == 'deleted'
    or git_status == 'staged_deleted'
    or git_status == 'renamed'
end

--- Resolve the appropriate git sign highlight for a row.
--- When the row is under the cursor, blends the git border foreground onto
--- the cursor highlight's background so the sign reads as part of the cursor line.
--- @param git_status string Git status
--- @param is_cursor boolean Whether the row is the current cursor row
--- @param cursor_hl string Cursor highlight group name
--- @return string Highlight group name (may be a generated group)
function M.get_git_sign_highlight(git_status, is_cursor, cursor_hl)
  if not is_cursor then return M.get_git_border_highlight(git_status) end

  local base_hl = M.get_git_border_highlight_selected(git_status)
  if not base_hl or base_hl == '' then return cursor_hl end

  local cached = git_cursor_border_cache[git_status]
  if cached then return cached end

  local base_id = vim.fn.synIDtrans(vim.fn.hlID(base_hl))
  local cursor_id = vim.fn.synIDtrans(vim.fn.hlID(cursor_hl))
  local border_fg_gui = vim.fn.synIDattr(base_id, 'fg', 'gui')
  local border_fg_cterm = vim.fn.synIDattr(base_id, 'fg', 'cterm')
  local cursor_bg_gui = vim.fn.synIDattr(cursor_id, 'bg', 'gui')
  local cursor_bg_cterm = vim.fn.synIDattr(cursor_id, 'bg', 'cterm')
  local has_gui = border_fg_gui ~= '' and cursor_bg_gui ~= ''
  local has_cterm = border_fg_cterm ~= '' and cursor_bg_cterm ~= ''

  if not has_gui and not has_cterm then
    git_cursor_border_cache[git_status] = base_hl
    return base_hl
  end

  local blended_name = 'FFFGitBorderSelected_' .. git_status
  local hl_opts = {}
  if has_gui then
    hl_opts.fg = border_fg_gui
    hl_opts.bg = cursor_bg_gui
  end
  if has_cterm then
    hl_opts.ctermfg = tonumber(border_fg_cterm)
    hl_opts.ctermbg = tonumber(cursor_bg_cterm)
  end
  vim.api.nvim_set_hl(0, blended_name, hl_opts)
  git_cursor_border_cache[git_status] = blended_name
  return blended_name
end

function M.setup()
  -- Reset caches so highlights pick up updated config / colorscheme values.
  git_text_highlights_cache = nil
  git_border_highlights_cache = nil
  git_border_highlights_selected_cache = nil
  git_cursor_border_cache = {}

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

    " File info panel highlights — defaults link to good fallbacks; users can override.
    " FFFFileInfoValue is set explicitly below (Normal's fg only) so it doesn't
    " import Normal's bg into the float (float content uses NormalFloat).
    highlight default link FFFFileInfoSection Title
    highlight default link FFFFileInfoSeparator FloatBorder
    highlight default link FFFFileInfoLabel Comment
    highlight default link FFFFileInfoValueDim NonText
    highlight default link FFFFileInfoSize Number
    highlight default link FFFFileInfoType Type
    highlight default link FFFFileInfoPath Directory
    highlight default link FFFFileInfoScorePos DiagnosticOk
    highlight default link FFFFileInfoScoreNeg DiagnosticError

    " Bold for the total score and the match-type tag.
    highlight default FFFFileInfoTotalScore gui=bold cterm=bold
    highlight default FFFFileInfoMatchType gui=bold cterm=bold
  ]])

  -- Resolve an attribute by walking the link chain.
  local function resolve_hl_attr(name, attr)
    local hl = vim.api.nvim_get_hl(0, { name = name, link = false })
    if hl[attr] ~= nil then return hl[attr] end
    local linked = vim.api.nvim_get_hl(0, { name = name })
    if linked.link then return resolve_hl_attr(linked.link, attr) end
    return nil
  end

  -- File info value text: copy Normal's fg only. Linking to Normal would
  -- carry Normal.bg into the float (whose content uses NormalFloat), creating
  -- visible patches behind plain values.
  local normal_fg = resolve_hl_attr('Normal', 'fg')
  if normal_fg then
    vim.api.nvim_set_hl(0, 'FFFFileInfoValue', { fg = normal_fg, default = true })
  else
    vim.api.nvim_set_hl(0, 'FFFFileInfoValue', { link = 'NormalFloat', default = true })
  end

  -- Resolve link target's fg so we can combine bold + colour (link + gui=bold
  -- can't be set in one nvim_set_hl call).
  local function bold_with_fallback(name, fallback_groups)
    for _, fb in ipairs(fallback_groups) do
      local hl = vim.api.nvim_get_hl(0, { name = fb, link = false })
      if hl and hl.fg then
        vim.api.nvim_set_hl(0, name, { fg = hl.fg, bold = true, default = true })
        return
      end
    end
  end
  bold_with_fallback('FFFFileInfoTotalScore', { 'Number', 'Constant', 'Identifier' })
  bold_with_fallback('FFFFileInfoMatchType', { 'Special', 'Statement', 'Keyword' })

  -- Highlights for git signs both for selected and normal states
  local git_highlights = {
    { 'FFFGitSignStaged', 'FFFGitSignStagedSelected', '#10B981', 2 },
    { 'FFFGitSignModified', 'FFFGitSignModifiedSelected', '#F59E0B', 3 },
    { 'FFFGitSignDeleted', 'FFFGitSignDeletedSelected', '#EF4444', 1 },
    { 'FFFGitSignRenamed', 'FFFGitSignRenamedSelected', '#8B5CF6', 5 },
    { 'FFFGitSignUntracked', 'FFFGitSignUntrackedSelected', '#10B981', 2 },
    { 'FFFGitSignIgnored', 'FFFGitSignIgnoredSelected', '#4B5563', 8 },
  }

  local visual_bg_gui = vim.fn.synIDattr(vim.fn.synIDtrans(vim.fn.hlID('Visual')), 'bg', 'gui')
  local visual_bg_cterm = vim.fn.synIDattr(vim.fn.synIDtrans(vim.fn.hlID('Visual')), 'bg', 'cterm')

  for _, hl in ipairs(git_highlights) do
    local _, selected_hl, gui_fg, cterm_fg = hl[1], hl[2], hl[3], hl[4]

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

  -- Selection highlight - use Directory/Number colors (better than green 'Added')
  vim.cmd('highlight default link FFFSelected Directory')

  local dir_fg_gui = vim.fn.synIDattr(vim.fn.synIDtrans(vim.fn.hlID('Directory')), 'fg', 'gui')
  local dir_fg_cterm = vim.fn.synIDattr(vim.fn.synIDtrans(vim.fn.hlID('Directory')), 'fg', 'cterm')

  if dir_fg_gui == '' or dir_fg_gui == '-1' then
    -- Directory not defined, try Number
    dir_fg_gui = vim.fn.synIDattr(vim.fn.synIDtrans(vim.fn.hlID('Number')), 'fg', 'gui')
    dir_fg_cterm = vim.fn.synIDattr(vim.fn.synIDtrans(vim.fn.hlID('Number')), 'fg', 'cterm')
  end

  -- Fallback to blue if neither Directory nor Number have colors
  local is_dark_bg = vim.o.background == 'dark'
  local gui_fg = dir_fg_gui ~= '' and dir_fg_gui or (is_dark_bg and '#60A5FA' or '#0369A1')
  local cterm_fg = dir_fg_cterm ~= '' and dir_fg_cterm or (is_dark_bg and '12' or '4')

  local gui_bg = visual_bg_gui ~= '' and visual_bg_gui or 'NONE'
  local cterm_bg = visual_bg_cterm ~= '' and visual_bg_cterm or 'NONE'

  -- Create combined highlight: Directory/Number foreground + Visual background
  vim.cmd(
    string.format(
      'highlight default FFFSelectedActive guifg=%s guibg=%s ctermfg=%s ctermbg=%s',
      gui_fg,
      gui_bg,
      cterm_fg,
      cterm_bg
    )
  )
end

return M
