<p align="center">
  <h1 align="center">FFF</h1>
</p>

<p align="center">
  <a href="#mcp"><strong>AI agents (MCP)</strong></a> &nbsp;&nbsp;|&nbsp;&nbsp; <a href="#neovim-guide"><strong>Neovim users</strong></a>
</p>

<p align="center">
  <i>A fast file search for your AI and neovim, with memory built-in</i>
</p>

<p align="center" style="text-decoration: none; border: none;">
	<a href="https://github.com/dmtrKovalenko/fff.nvim/stargazers" style="text-decoration: none">
		<img alt="Stars" src="https://img.shields.io/github/stars/dmtrKovalenko/fff.nvim?style=for-the-badge&logo=starship&color=C9CBFF&logoColor=D9E0EE&labelColor=302D41"></a>
	<a href="https://github.com/dmtrKovalenko/fff.nvim/issues" style="text-decoration: none">
		<img alt="Issues" src="https://img.shields.io/github/issues/dmtrKovalenko/fff.nvim?style=for-the-badge&logo=bilibili&color=F5E0DC&logoColor=D9E0EE&labelColor=302D41"></a>
	<a href="https://github.com/dmtrKovalenko/fff.nvim/contributors" style="text-decoration: none"> <img alt="Contributors" src="https://img.shields.io/github/contributors/dmtrKovalenko/fff.nvim?color=%23DDB6F2&label=CONTRIBUTORS&logo=git&style=for-the-badge&logoColor=D9E0EE&labelColor=302D41"/></a>
</p>

---

**FFF** stands for ~~freakin fast fuzzy file finder~~ (pick 3) and it is an opinionated fuzzy file picker for your AI agent and Neovim. Just for file search, but we do the file search really fff well.

FFF is a tool for grepping, fuzzy file matching, globbing, and multigrepping with a strong focus on performance and useful search results. For humans - provides an unbelievable typo-resistant experience, for AI agents - implements the fastest file search with additional free memory suggesting the best search results based on various factors like frecency, git status, file size, definition matches, and more.

## MCP

FFF is an amazing way to reduce the time and tokens by giving your AI agent a bit of memory built-in to their file search tools. It makes your AI harness to find the code faster and spend less tokens by doing less roundtrips and reading less useless files.

![Chart showing the superiority of fff.nvim over builtin claude code tools](./chart.png)

You can install FFF as a dependency for your AI agent using a simple bash script:

```bash
curl -L https://dmtrkovalenko.dev/install-fff-mcp.sh | bash
```

> The installation script is here [./install-mcp.sh](./install-mcp.sh) if you want to review it before running.

It will print out the instructions on how to connect it to your `Claude Code`, `Codex`, `OpenCode`, etc. Once you have it connected just ask your agent to "use fff".
Here is an example addition to `CLAUDE.md` that works perfectly:

```sh
# CLAUDE.md
For any file search or grep in the current git indexed directory use fff tools
```

## Neovim guide

Here is some demo on the linux repository (100k files, 8GB) but you better fill it yourself and see the magic

https://github.com/user-attachments/assets/5d0e1ce9-642c-4c44-aa88-01b05bb86abb

### Installation

FFF.nvim requires neovim 0.10.0 or higher

#### lazy.nvim

```lua
{
  'dmtrKovalenko/fff.nvim',
  build = function()
    -- this will download prebuild binary or try to use existing rustup toolchain to build from source
    -- (if you are using lazy you can use gb for rebuilding a plugin if needed)
    require("fff.download").download_or_build_binary()
  end,
  -- if you are using nixos
  -- build = "nix run .#release",
  opts = { -- (optional)
    debug = {
      enabled = true,     -- we expect your collaboration at least during the beta
      show_scores = true, -- to help us optimize the scoring system, feel free to share your scores!
    },
  },
  -- No need to lazy-load with lazy.nvim.
  -- This plugin initializes itself lazily.
  lazy = false,
  keys = {
    {
      "ff", -- try it if you didn't it is a banger keybinding for a picker
      function() require('fff').find_files() end,
      desc = 'FFFind files',
    },
    {
      "fg",
      function() require('fff').live_grep() end,
      desc = 'LiFFFe grep',
    },
    {
      "fz",
      function() require('fff').live_grep({
        grep = {
          modes = { 'fuzzy', 'plain' }
        }
      }) end,
      desc = 'Live fffuzy grep',
    },
    {
      "fc",
      function() require('fff').live_grep({ query = vim.fn.expand("<cword>") }) end,
      desc = 'Search current word',
    },
  }
}
```

#### vim.pack

```lua
vim.pack.add({ 'https://github.com/dmtrKovalenko/fff.nvim' })

vim.api.nvim_create_autocmd('PackChanged', {
  callback = function(ev)
    local name, kind = ev.data.spec.name, ev.data.kind
    if name == 'fff.nvim' and (kind == 'install' or kind == 'update') then
      if not ev.data.active then
        vim.cmd.packadd('fff.nvim')
      end
      require('fff.download').download_or_build_binary()
    end
  end,
})

-- the plugin will automatically lazy load
vim.g.fff = {
  lazy_sync = true, -- start syncing only when the picker is open
  debug = {
    enabled = true,
    show_scores = true,
  },
}

vim.keymap.set(
  'n',
  'ff',
  function() require('fff').find_files() end,
  { desc = 'FFFind files' }
)
```

### Configuration

FFF.nvim comes with sensible defaults. Here's the complete configuration with all available options:

```lua
require('fff').setup({
    base_path = vim.fn.getcwd(),
    prompt = '🪿 ',
    title = 'FFFiles',
    max_results = 100,
    max_threads = 4,
    lazy_sync = true, -- set to false if you want file indexing to start on open
    prompt_vim_mode = false, -- set to true to enable vim-mode in the prompt: <Esc> leaves insert for normal mode bindings (also allows <leader>p or <leader>l to jump around) the second <Esc> closes the picker
    layout = {
      height = 0.8,
      width = 0.8,
      prompt_position = 'bottom', -- or 'top'
      preview_position = 'right', -- or 'left', 'right', 'top', 'bottom'
      preview_size = 0.5,
      flex = { -- set to false to disable flex layout
        size = 130, -- column threshold: if screen width >= size, use preview_position; otherwise use wrap
        wrap = 'top', -- position to use when screen is narrower than size
      },
      show_scrollbar = true, -- Show scrollbar for pagination
      -- How to shorten long directory paths in the file list:
      -- 'middle_number' (default): uses dots for 1-3 hidden (a/./b, a/../b, a/.../b)
      --                            and numbers for 4+ (a/.4./b, a/.5./b)
      -- 'middle': always uses dots (a/./b, a/../b, a/.../b)
      -- 'end': truncates from the end (home/user/projects)
      path_shorten_strategy = 'middle_number',
    },
    preview = {
      enabled = true,
      max_size = 10 * 1024 * 1024, -- Do not try to read files larger than 10MB
      chunk_size = 8192, -- Bytes per chunk for dynamic loading (8kb - fits ~100-200 lines)
      binary_file_threshold = 1024, -- amount of bytes to scan for binary content (set 0 to disable)
      imagemagick_info_format_str = '%m: %wx%h, %[colorspace], %q-bit',
      line_numbers = false,
      cursorlineopt = 'both', -- the cursorlineopt used for lines in grep file previews, see :h cursorlineopt
      wrap_lines = false,
      filetypes = {
        svg = { wrap_lines = true },
        markdown = { wrap_lines = true },
        text = { wrap_lines = true },
      },
    },
    keymaps = {
      close = '<Esc>',
      select = '<CR>',
      select_split = '<C-s>',
      select_vsplit = '<C-v>',
      select_tab = '<C-t>',
      -- you can assign multiple keys to any action
      move_up = { '<Up>', '<C-p>' },
      move_down = { '<Down>', '<C-n>' },
      preview_scroll_up = '<C-u>',
      preview_scroll_down = '<C-d>',
      toggle_debug = '<F2>',
      -- grep mode: cycle between plain text, regex, and fuzzy search
      cycle_grep_modes = '<S-Tab>',
      -- goes to the previous query in history
      cycle_previous_query = '<C-Up>',
      -- multi-select keymaps for quickfix
      toggle_select = '<Tab>',
      send_to_quickfix = '<C-q>',
      -- this are specific for the normal mode (you can exit it using any other keybind like jj)
      focus_list = '<leader>l',
      focus_preview = '<leader>p',
    },
    hl = {
      border = 'FloatBorder',
      normal = 'Normal',
      cursor = 'CursorLine',  -- Falls back to 'Visual' if CursorLine is not defined
      matched = 'IncSearch',
      title = 'Title',
      prompt = 'Question',
      frecency = 'Number',
      debug = 'Comment',
      combo_header = 'Number',
      scrollbar = 'Comment',
      directory_path = 'Comment',
      -- Multi-select highlights
      selected = 'FFFSelected',
      selected_active = 'FFFSelectedActive',
      -- Git text highlights for file names
      git_staged = 'FFFGitStaged',
      git_modified = 'FFFGitModified',
      git_deleted = 'FFFGitDeleted',
      git_renamed = 'FFFGitRenamed',
      git_untracked = 'FFFGitUntracked',
      git_ignored = 'FFFGitIgnored',
      -- Git sign/border highlights
      git_sign_staged = 'FFFGitSignStaged',
      git_sign_modified = 'FFFGitSignModified',
      git_sign_deleted = 'FFFGitSignDeleted',
      git_sign_renamed = 'FFFGitSignRenamed',
      git_sign_untracked = 'FFFGitSignUntracked',
      git_sign_ignored = 'FFFGitSignIgnored',
      -- Git sign selected highlights
      git_sign_staged_selected = 'FFFGitSignStagedSelected',
      git_sign_modified_selected = 'FFFGitSignModifiedSelected',
      git_sign_deleted_selected = 'FFFGitSignDeletedSelected',
      git_sign_renamed_selected = 'FFFGitSignRenamedSelected',
      git_sign_untracked_selected = 'FFFGitSignUntrackedSelected',
      git_sign_ignored_selected = 'FFFGitSignIgnoredSelected',
      -- Grep highlights
      grep_match = 'IncSearch',               -- Highlight for matched text in grep results
      grep_line_number = 'LineNr',            -- Highlight for :line:col location
      grep_regex_active = 'DiagnosticInfo',   -- Highlight for keybind + label when regex is on
      grep_plain_active = 'Comment',        -- Highlight for keybind + label when regex is off
      grep_fuzzy_active = 'DiagnosticHint',   -- Highlight for keybind + label when fuzzy is on
      -- Cross-mode suggestion highlights
      suggestion_header = 'WarningMsg', -- Highlight for the "No results found. Suggested..." banner
    },
    -- Store file open frecency
    frecency = {
      enabled = true,
      db_path = vim.fn.stdpath('cache') .. '/fff_nvim',
    },
    -- Store successfully opened queries with respective matches
    history = {
      enabled = true,
      db_path = vim.fn.stdpath('data') .. '/fff_queries',
      min_combo_count = 3, -- Minimum selections before combo boost applies (3 = boost starts on 3rd selection)
      combo_boost_score_multiplier = 100, -- Score multiplier for combo matches (files repeatedly opened with same query)
    },
    -- Git integration
    git = {
      status_text_color = false, -- Apply git status colors to filename text (default: false, only sign column)
    },
    debug = {
      enabled = false, -- Show file info panel in preview
      show_scores = false, -- Show scores inline in the UI
    },
    logging = {
      enabled = true,
      log_file = vim.fn.stdpath('log') .. '/fff.log',
      log_level = 'info',
    },
    -- find_files settings
    file_picker = {
      current_file_label = '(current)',
    },
    -- grep settings
    grep = {
      max_file_size = 10 * 1024 * 1024, -- Skip files larger than 10MB
      max_matches_per_file = 100, -- Maximum matches per file (set 0 to unlimited)
      smart_case = true, -- Case-insensitive unless query has uppercase
      time_budget_ms = 150, -- Max search time in ms per call (prevents UI freeze, 0 = no limit)
      modes = { 'plain', 'regex', 'fuzzy' }, -- Available grep modes and their cycling order
      trim_whitespace = false, -- Strip leading whitespace from matched lines
    },
  })
```

### Key Features

#### Available Methods

```lua
require('fff').find_files()                         -- Find files in current repository
require('fff').scan_files()                         -- Trigger rescan of files in the current directory
require('fff').refresh_git_status()                 -- Refresh git status for the active file list
require('fff').find_files_in_dir(path)              -- Find files in a specific directory
require('fff').change_indexing_directory(new_path)  -- Change the base directory for the file picker
```

just jump to the definition and see what other APIs are exposed we have a plenty

#### Commands

FFF.nvim provides several commands for interacting with the file picker:

- `:FFFScan` - Manually trigger a rescan of files in the current directory
- `:FFFRefreshGit` - Manually refresh git status for all files
- `:FFFClearCache [all|frecency|files]` - Clear various caches
- `:FFFHealth` - Check FFF health status and dependencies
- `:FFFDebug [on|off|toggle]` - Toggle debug scores display
- `:FFFOpenLog` - Open the FFF log file in a new tab

#### Debug Mode

Toggle scoring information display:

- Press `F2` while in the picker
- Use `:FFFDebug` command
- Enable by default with `debug.show_scores = true`

#### Multi-Select and Quickfix Integration

Select multiple files and send them to Neovim's quickfix list (keymaps are configurable):

- `<Tab>` - Toggle selection for the current file (shows thick border `▊` in signcolumn)
- `<C-q>` - Send selected files to quickfix list and close picker

#### Live Grep Search Modes

Live grep supports three search modes, cycled with `<S-Tab>`:

- **Plain text** (default) - The query is matched literally. Special regex characters like `.`, `*`, `(`, `)`, `$` have no special meaning. This is the safest mode for searching code containing regex metacharacters.
- **Regex** - The query is interpreted as a regular expression. Supports character classes (`[a-z]`), quantifiers (`+`, `*`, `{n}`), alternation (`foo|bar`), anchors (`^`, `$`), word boundaries (`\b`), and more.
- **Fuzzy** - The query is fuzzy matched using Smith-Waterman scoring. Accommodates typos and scattered characters (e.g., "mtxlk" matches "mutex_lock"). Results are filtered by a quality threshold to avoid overly fuzzy matches.

The current mode is shown on the right side of the input field (e.g., `plain`, `regex`, `fuzzy`) with color-coded highlighting.

You can customize which modes are available and their cycling order globally in your configuration, or per-call when invoking `live_grep()`.

**Global configuration:**

```lua
require('fff').setup({
  grep = {
    modes = { 'plain', 'regex' }, -- Only plain and regex, no fuzzy
  }
})
```

**Per-call configuration:**

```lua
-- Only fuzzy and plain modes for this specific grep
require('fff').live_grep({
  grep = {
    modes = { 'fuzzy', 'plain' },
  }
})

-- Single mode (hides mode indicator completely)
require('fff').live_grep({
  grep = {
    modes = { 'fuzzy' },
  }
})

-- Pre-fill the search with an initial query
require('fff').live_grep({ query = 'search term' })
```

When only one mode is configured, the mode indicator is hidden completely and the cycle keybind does nothing.

#### Constraints

There are a number of constraints you can use to refine your search in both grep and file search mode:

- `git:modified` - show only modified files (one of `modified`, `staged`, `deleted`, `renamed`, `untracked`, `ignored`)
- `test/` - any deeply nested children of any test/ dir
- `!something` - exclude results matching something
- `!test/`, `!git:modified` - combining with any other constraint works as negation
- `./**/*.{rs,lua}` - any valid glob expression via [the fastest globbing library](https://github.com/dmtrKovalenko/zlob)

For grep only:

- `*.md`, `*.{c,h}` - extension filtering
- `src/main.rs` - grep in a single file

In addition to that, all constraints can be combined together like:

```
git:modified src/**/*.rs !src/**/mod.rs user controller
```

This will find all the files that qualify the constraints and:

- match **both** user and controller (for file mode)
- match "user controller" (for grep mode)

#### Cross-Mode Suggestions

When a search returns no results, FFF automatically queries the opposite search mode and displays the results as suggestions:

- **File search with no matches** → shows suggested **content matches** (grep results) for the same query
- **Grep search with no matches** → shows suggested **file name matches** for the same query

Suggestions are clearly labeled with a "No results found. Suggested ..." banner (highlighted with `hl.suggestion_header`). You can navigate and select suggestion items just like normal results — selecting a grep suggestion will open the file at the matching line.

#### Git Status Highlighting

FFF integrates with git to show file status through sign column indicators (enabled by default) and optional filename text coloring.

**Sign Column Indicators** (enabled by default) - Border characters shown in the sign column:

```lua
hl = {
  git_sign_staged = 'FFFGitSignStaged',
  git_sign_modified = 'FFFGitSignModified',
  git_sign_deleted = 'FFFGitSignDeleted',
  git_sign_renamed = 'FFFGitSignRenamed',
  git_sign_untracked = 'FFFGitSignUntracked',
  git_sign_ignored = 'FFFGitSignIgnored',
}
```

**Text Highlights** (opt-in) - Apply colors to filenames based on git status:

To enable git status text coloring, set `git.status_text_color = true`:

```lua
require('fff').setup({
  git = {
    status_text_color = true, -- Enable git status colors on filename text
  },
  hl = {
    git_staged = 'FFFGitStaged',       -- Files staged for commit
    git_modified = 'FFFGitModified',   -- Modified unstaged files
    git_deleted = 'FFFGitDeleted',     -- Deleted files
    git_renamed = 'FFFGitRenamed',     -- Renamed files
    git_untracked = 'FFFGitUntracked', -- New untracked files
    git_ignored = 'FFFGitIgnored',     -- Git-ignored files
  }
})
```

The plugin provides sensible default highlight groups that link to common git highlight groups (e.g., GitSignsAdd, GitSignsChange). You can override these with your own custom highlight groups to match your colorscheme.

**Example - Custom Bright Colors for Text:**

```lua
vim.api.nvim_set_hl(0, 'CustomGitModified', { fg = '#FFA500' })
vim.api.nvim_set_hl(0, 'CustomGitUntracked', { fg = '#00FF00' })

require('fff').setup({
  git = {
    status_text_color = true,
  },
  hl = {
    git_modified = 'CustomGitModified',
    git_untracked = 'CustomGitUntracked',
  }
})
```

#### File Filtering

FFF.nvim respects `.gitignore` patterns automatically. To filter files from the picker without modifying `.gitignore`, create a `.ignore` file in your project root:

```gitignore
# Exclude all markdown files
*.md

# Exclude specific subdirectory
docs/archive/**/*.md
```

Run `:FFFScan` to force a rescan if needed.

### Troubleshooting

#### Health Check

Run `:FFFHealth` to check the status of FFF.nvim and its dependencies. This will verify:

- File picker initialization status
- Optional dependencies (git, image preview tools)
- Database connectivity

#### Viewing Logs

If you encounter issues, check the log file:

```
:FFFOpenLog
```

Or manually open the log file at `~/.local/state/nvim/log/fff.log` (default location).
