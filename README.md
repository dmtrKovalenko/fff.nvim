<p align="center">
  <h2 align="center">FFF.nvim</h2>
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
- Git status integration allowing to take adavantage of last modified times within a worktree
- Separate file index maintained by a dedicaged backend allows <10 milliseconds search time for 50k files codebase
- Display images in previews (for now requires snacks.nvim)
- Smart in a plenty of different ways hopefully helpful for your workflow

## Installation

> [!NOTE]  
> Although we'll try to make sure to keep 100% backward compatibiility, by using you should understand that silly bugs and breaking changes may happen.
> And also we hope for your contributions and feedback to make this plugin ideal for everyone.

### Prerequisites

FFF.nvim requires:

- Neovim 0.10.0+
- Rust toolchain (requires nightly for building the native backend)

### Package Managers

#### lazy.nvim

```lua
{
  "dmtrKovalenko/fff.nvim",
  build = "cargo build --release",
  -- or if you are using nixos
  -- build = "nix run .#release",
  opts = {
    -- pass here all the options
  },
  keys = {
    {
      "ff", -- try it if you didn't it is a banger keybinding for a picker
      function()
        require("fff").find_files() -- or find_in_git_root() if you only want git files
      end,
      desc = "Open file picker",
    },
  },
}
```

### Default Configuration

FFF.nvim comes with sensible defaults. Here's the complete default configuration:

```lua
require('fff').setup({
  prompt = '🪿 ',
  title = 'FFF Files',
  max_results = 100, -- Maximum search results to display
  max_threads = 4, -- Maximum threads for fuzzy search

  preview = {
    enabled = true,
    max_lines = 100,
    max_size = 1024 * 1024, -- 1MB
  },

  layout = {
    height = 0.8, -- Window height as fraction of screen
    width = 0.8, -- Window width as fraction of screen
    prompt_position = 'bottom', -- 'top' or 'bottom'
    preview_position = 'right', -- 'left', 'right', 'top', 'bottom'
    preview_size = 0.4, -- Preview size as fraction of window (width for left/right, height for top/bottom)
  },

  keymaps = {
    close = '<Esc>',
    select = '<CR>',
    select_split = '<C-s>',
    select_vsplit = '<C-v>',
    select_tab = '<C-t>',
    -- Multiple bindings supported
    move_up = { '<Up>', '<C-p>' },
    move_down = { '<Down>', '<C-n>' },
    preview_scroll_up = '<C-u>',
    preview_scroll_down = '<C-d>',
  },

  -- Highlight groups
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
  },

  -- Debug options
  debug = {
    show_scores = false, -- Toggle with F2 or :FFFDebug
  },
})
```

### Key Features

#### Available methods

```lua
require('fff').find_files() -- Find files in current directory
require('fff').find_in_git_root() -- Find files in the current git repository
require('fff').scan_files() -- Trigger rescan of files in the current directory
require('fff').refresh_git_status() -- Refresh git status for the active file lock
require('fff').find_files_in_dir(path) -- Find files in a specific directory
require('fff').change_indexing_directory(new_path) -- Change the base directory for the file picker
```

#### Multiple Key Bindings

You can assign multiple key combinations to the same action:

```lua
keymaps = {
  move_up = { '<Up>', '<C-p>', '<C-k>' }, -- Three ways to move up
  close = { '<Esc>', '<C-c>' }, -- Two ways to close
  select = '<CR>', -- Single binding still works
}
```

#### Multiline Paste Support

The input field automatically handles multiline clipboard content by joining all lines into a single search query. This is particularly useful when copying file paths from terminal output.

#### Debug Mode

Toggle scoring information display:

- Press `F2` while in the picker
- Use `:FFFDebug` command
- Enable by default with `debug.show_scores = true`

````

#### vim-plug

```vim
Plug 'MunifTanjim/nui.nvim'
Plug 'dmtrKovalenko/fff.nvim', { 'do': 'cargo build --release' }
````
