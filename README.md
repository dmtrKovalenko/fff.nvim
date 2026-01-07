<p align="center">
  <h2 align="center">FFF.nvim + Buffers Support</h2>
</p>

<p align="center">
	Finally a smart fuzzy file picker for neovim.
</p>

<p align="center" style="text-decoration: none; border: none;">
	<a href="https://github.com/dmtrKovalenko/fff.nvim/stargazers" style="text-decoration: none">
		<img alt="Stars" src="https://img.shields.io/github/stars/dmtrKovalenko/fff.nvim?style=for-the-badge&logo=starship&color=C9CBFF&logoColor=D9E0EE&labelColor=302D41"></a>
	<a href="https://github.com/dmtrKovalenko/fff.nvim/issues" style="text-decoration: none">
		<img alt="Issues" src="https://img.shields.io/github/issues/dmtrKovalenko/fff.nvim?style=for-the-badge&logo=bilibili&color=F5E0DC&logoColor=D9E0EE&labelColor=302D41"></a>
	<a href="https://github.com/dmtrKovalenko/fff.nvim/contributors" style="text-decoration: none">
		<img alt="Contributors" src="https://img.shields.io/github/contributors/dmtrKovalenko/fff.nvim?color=%23DDB6F2&label=CONTRIBUTORS&logo=git&style=for-the-badge&logoColor=D9E0EE&labelColor=302D41"/></a>
</p>

**FFF** stands for ~freakin fast fuzzy file finder~ (pick 3) and it is an opinionated fuzzy file picker for neovim. Just for files, but we'll try to solve file picking completely.

It comes with a dedicated rust backend runtime that keep tracks of the file index, your file access and modifications, git status, and provides a comprehensive typo-resistant fuzzy search experience.

## Features

- Works out of the box with no additional configuration
- [Typo resistant fuzzy search](https://github.com/saghen/frizbee)
- Git status integration allowing to take advantage of last modified times within a worktree
- Separate file index maintained by a dedicated backend allows <10 milliseconds search time for 50k files codebase
- **Buffer picker** - quickly switch between open buffers (similar to fzf.vim's `:Buffers`)
- **Colors picker** - browse and switch colorschemes with live preview (similar to fzf.vim's `:Colors`)
- Display images in previews (for now requires snacks.nvim)
- Smart in a plenty of different ways hopefully helpful for your workflow
- This plugin initializes itself lazily by default

## Installation

> [!NOTE]
> Although we'll try to make sure to keep 100% backward compatibility, by using you should understand that silly bugs and breaking changes may happen.
> And also we hope for your contributions and feedback to make this plugin ideal for everyone.

### Prerequisites

FFF.nvim requires:

- Neovim 0.10.0+
- [Rustup](https://rustup.rs/) (we require nightly for building the native backend rustup will handle toolchain automatically)

### Installation

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
      "<leader>b",
      function() require('fff').buffers() end,
      desc = 'FFF Buffers',
    },
    {
      "<leader>c",
      function() require('fff').colors() end,
      desc = 'FFF Colors',
    },
  }
}
```

#### vim.pack

```lua
vim.pack.add({ 'https://github.com/dmtrKovalenko/fff.nvim' })

vim.api.nvim_create_autocmd('PackChanged', {
  callback = function(event)
    if event.data.updated then
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

vim.keymap.set(
  'n',
  '<leader>b',
  function() require('fff').buffers() end,
  { desc = 'FFF Buffers' }
)

vim.keymap.set(
  'n',
  '<leader>c',
  function() require('fff').colors() end,
  { desc = 'FFF Colors' }
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
    layout = {
      height = 0.8,
      width = 0.8,
      prompt_position = 'bottom', -- or 'top'
      preview_position = 'right', -- or 'left', 'right', 'top', 'bottom'
      preview_size = 0.5,
      show_scrollbar = true, -- Show scrollbar for pagination
    },
    preview = {
      enabled = true,
      max_size = 10 * 1024 * 1024, -- Do not try to read files larger than 10MB
      chunk_size = 8192, -- Bytes per chunk for dynamic loading (8kb - fits ~100-200 lines)
      binary_file_threshold = 1024, -- amount of bytes to scan for binary content (set 0 to disable)
      imagemagick_info_format_str = '%m: %wx%h, %[colorspace], %q-bit',
      line_numbers = false,
      wrap_lines = false,
      show_file_info = true,
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
      -- goes to the previous query in history
      cycle_previous_query = '<C-Up>',
      -- multi-select keymaps for quickfix
      toggle_select = '<Tab>',
      send_to_quickfix = '<C-q>',
    },
    hl = {
      border = 'FloatBorder',
      normal = 'Normal',
      cursor = 'CursorLine',
      matched = 'IncSearch',
      title = 'Title',
      prompt = 'Question',
      active_file = 'Visual',
      frecency = 'Number',
      debug = 'Comment',
      combo_header = 'Number',
      scrollbar = 'Comment', -- Highlight for scrollbar thumb (track uses border)
      directory_path = 'Comment', -- Highlight for directory path in file list
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
      min_combo_count = 3, -- file will get a boost if it was selected 3 in a row times per specific query
      combo_boost_score_multiplier = 100, -- Score multiplier for combo matches
    },
    -- Git integration
    git = {
      status_text_color = false, -- Apply git status colors to filename text (default: false, only sign column)
    },
    debug = {
      enabled = false, -- Set to true to show scores in the UI
      show_scores = false,
    },
    logging = {
      enabled = true,
      log_file = vim.fn.stdpath('log') .. '/fff.log',
      log_level = 'info',
    }
})
```

### Key Features

#### Available Methods

```lua
require('fff').find_files()                         -- Find files in current directory
require('fff').find_in_git_root()                   -- Find files in the current git repository
require('fff').buffers()                            -- Open buffer picker (similar to fzf.vim :Buffers)
require('fff').colors()                             -- Open colors picker (similar to fzf.vim :Colors)
require('fff').scan_files()                         -- Trigger rescan of files in the current directory
require('fff').refresh_git_status()                 -- Refresh git status for the active file lock
require('fff').find_files_in_dir(path)              -- Find files in a specific directory
require('fff').change_indexing_directory(new_path)  -- Change the base directory for the file picker
```

#### Commands

FFF.nvim provides several commands for interacting with the file picker:

- `:FFFFind [path|query]` - Open file picker. Optional: provide directory path or search query
- `:FFFBuffers` - Open buffer picker to browse and switch between open buffers
- `:Colors` - Open colors picker to browse and switch colorschemes with live preview
- `:FFFScan` - Manually trigger a rescan of files in the current directory
- `:FFFRefreshGit` - Manually refresh git status for all files
- `:FFFClearCache [all|frecency|files]` - Clear various caches
- `:FFFHealth` - Check FFF health status and dependencies
- `:FFFDebug [on|off|toggle]` - Toggle debug scores display
- `:FFFOpenLog` - Open the FFF log file in a new tab

#### Buffer Picker

The buffer picker (`:FFFBuffers`) provides a fast way to switch between open buffers, similar to fzf.vim's `:Buffers` command.

**Features:**
- Buffers sorted by most recently accessed
- Status indicators: `%` for current buffer, `#` for alternate buffer
- Shows `[+]` for modified buffers and `[RO]` for read-only buffers
- File icons via nvim-web-devicons
- Preview of buffer contents
- Delete buffers with `<C-d>`

**Keybindings in buffer picker:**

| Key | Action |
|-----|--------|
| `<CR>` | Open buffer in current window |
| `<C-s>` | Open buffer in horizontal split |
| `<C-v>` | Open buffer in vertical split |
| `<C-t>` | Open buffer in new tab |
| `<C-d>` | Delete the selected buffer |
| `<Esc>` | Close picker |
| `<Up>` / `<C-p>` | Move selection up |
| `<Down>` / `<C-n>` | Move selection down |

**Example keybinding:**

```lua
vim.keymap.set('n', '<leader>b', function() require('fff').buffers() end, { desc = 'FFF Buffers' })
```

#### Colors Picker

The colors picker (`:Colors`) provides a fast way to browse and switch colorschemes, similar to fzf.vim's `:Colors` command.

**Features:**
- Lists all available colorschemes from runtimepath and packages
- Live preview as you navigate (colorscheme changes while browsing)
- Current colorscheme shown at the top with `*` indicator
- Fuzzy search to filter colorschemes
- Restores original colorscheme if you cancel (press `<Esc>`)

**Keybindings in colors picker:**

| Key | Action |
|-----|--------|
| `<CR>` | Apply selected colorscheme |
| `<Esc>` | Cancel and restore original colorscheme |
| `<Up>` / `<C-p>` | Move selection up (and preview) |
| `<Down>` / `<C-n>` | Move selection down (and preview) |

**Example keybinding:**

```lua
vim.keymap.set('n', '<leader>c', function() require('fff').colors() end, { desc = 'FFF Colors' })
```

#### Multiple Key Bindings

You can assign multiple key combinations to the same action:

```lua
keymaps = {
  move_up = { '<Up>', '<C-p>', '<C-k>' },  -- Three ways to move up
  close = { '<Esc>', '<C-c>' },            -- Two ways to close
  select = '<CR>',                         -- Single binding still works
}
```

#### Multiline Paste Support

The input field automatically handles multiline clipboard content by joining all lines into a single search query. This is particularly useful when copying file paths from terminal output.

#### Debug Mode

Toggle scoring information display:

- Press `F2` while in the picker
- Use `:FFFDebug` command
- Enable by default with `debug.show_scores = true`

#### Multi-Select and Quickfix Integration

Select multiple files and send them to Neovim's quickfix list (keymaps are configurable):

- `<Tab>` - Toggle selection for the current file (shows thick border `▊` in signcolumn)
- `<C-q>` - Send selected files to quickfix list and close picker

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


### Troubleshooting

#### Health Check

Run `:FFFHealth` to check the status of FFF.nvim and its dependencies. This will verify:

- File picker initialization status
- Optional dependencies (git, image preview tools)
- Database connectivity

#### Viewing Logs

If you encounter issues, check the log file:

```vim
:FFFOpenLog
```

Or manually open the log file at `~/.local/state/nvim/log/fff.log` (default location).

#### Common Issues

**File picker not initializing:**

- Ensure the Rust backend is compiled: `cargo build --release` in the plugin directory
- Check that your Neovim version is 0.10.0 or higher

**Image previews not working:**

- Verify your terminal supports images (kitty, iTerm2, WezTerm, etc.)
- For terminals without native image support, install one of: `chafa`, `viu`, or `img2txt`
- If using snacks.nvim, ensure it's properly configured

**Performance issues:**

- Adjust `max_threads` in configuration based on your system
- Reduce `preview.max_lines` and `preview.max_size` for large files
- Clear cache if it becomes too large: `:FFFClearCache all`

**Files not being indexed:**

- Run `:FFFScan` to manually trigger a file scan
- Check that the `base_path` is correctly set
- Verify you have read permissions for the directory

#### Debug Mode

Enable debug mode to see scoring information and troubleshoot search results:

- Press `F2` while in the picker
- Run `:FFFDebug on` to enable permanently
- Set `debug.show_scores = true` in configuration
