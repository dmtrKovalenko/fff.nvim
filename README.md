<img alt="f3+ logo for fff-plus.nvim" src="./assets/logo-f3-plus-orange.png" width="300">

# fff-plus.nvim

Extra Neovim pickers for [fff.nvim](https://github.com/dmtrKovalenko/fff.nvim).

`fff-plus.nvim` is a small extension plugin. Upstream `fff.nvim` keeps owning
the Rust backend, binary downloader, file search, live grep, frecency index, and
release pipeline. This plugin adds the picker workflows that are useful day to
day but do not need to live in the backend project.

## Usage demo

Tracked-file fuzzy search, Git-status diff preview and multi-select, quickfix,
and the fullscreen buffer picker are shown below. Click the preview for the
MP4 version.

[![fff-plus.nvim picker usage demo](./assets/fff-plus-usage.gif)](./assets/fff-plus-usage.mp4)

## Installation

Install upstream `fff.nvim` first, then install `fff-plus.nvim`.

### lazy.nvim

```lua
{
  'dmtrKovalenko/fff.nvim',
  build = function()
    require('fff.download').download_or_build_binary()
  end,
  lazy = false,
  keys = {
    { '<C-p>', function() require('fff').find_files() end, desc = 'FFF files' },
    { 'fg', function() require('fff').live_grep() end, desc = 'FFF grep' },
  },
},
{
  'vinitkumar/fff-plus.nvim',
  dependencies = { 'dmtrKovalenko/fff.nvim' },
  opts = {
    legacy_commands = false,
  },
  keys = {
    { '<C-b>', function() require('fff_plus').buffers() end, desc = 'FFF+ buffers' },
    { '<leader>c', function() require('fff_plus').colors() end, desc = 'FFF+ colors' },
    { '<leader>g', function() require('fff_plus').tracked_files() end, desc = 'FFF+ tracked files' },
    { '<leader>s', function() require('fff_plus').git_status() end, desc = 'FFF+ git status' },
  },
}
```

### vim.pack

```lua
vim.pack.add({
  'https://github.com/dmtrKovalenko/fff.nvim',
  'https://github.com/vinitkumar/fff-plus.nvim',
})

vim.api.nvim_create_autocmd('PackChanged', {
  callback = function(ev)
    local name, kind = ev.data.spec.name, ev.data.kind
    if name == 'fff.nvim' and (kind == 'install' or kind == 'update') then
      if not ev.data.active then vim.cmd.packadd('fff.nvim') end
      require('fff.download').download_or_build_binary()
    end
  end,
})

require('fff_plus').setup({
  legacy_commands = false,
})
```

## Pickers

| API | Command | What it does |
| --- | --- | --- |
| `require('fff_plus').buffers()` | `:FFFPlusBuffers` | Switch between listed buffers, preview contents, and delete buffers |
| `require('fff_plus').colors()` | `:FFFPlusColors` | Browse colorschemes with live preview and restore on cancel |
| `require('fff_plus').tracked_files()` | `:FFFPlusGitFiles` | Browse files tracked by Git |
| `require('fff_plus').git_status()` | `:FFFPlusGitStatus` | Browse changed files with status indicators and diff preview |
| `require('fff_plus').git_files()` | `:FFFPlusGFiles` | Compatibility API for the Git-status picker |

All pickers support ranked fuzzy matching and keep long result lists scrolled to
the selected item. Add `!` to an extension command to use the fullscreen
layout, for example `:FFFPlusBuffers!`.

Buffers and both Git pickers support these actions using upstream FFF keymap
defaults:

| Action | Default key |
| --- | --- |
| Toggle selection | `<Tab>` |
| Send selected/current entries to quickfix | `<C-q>` |
| Open in split / vertical split / tab | `<C-s>` / `<C-v>` / `<C-t>` |
| Paste selected/current paths into the invoking buffer | `<A-CR>` |

Override the paste key per call, or make the buffer picker jump to a window
that already displays the selected buffer:

```lua
require('fff_plus').buffers({
  jump_to_existing = true,
  keymaps = { paste = '<M-p>' },
})
```

## Compatibility Aliases

Set `legacy_commands = true` to also register aliases for muscle memory from
fzf.vim-style workflows:

```lua
require('fff_plus').setup({
  legacy_commands = true,
})
```

That enables:

- `:FFFBuffers`
- `:Colors`
- `:GFiles` (tracked files, matching fzf.vim semantics)

The older `:FFFPlusGFiles` command and `git_files()` Lua API remain aliases for
the Git-status workflow. Prefer the explicit `FFFPlusGitFiles`/
`tracked_files()` and `FFFPlusGitStatus`/`git_status()` names in new config.

If you want old Lua call sites such as `require('fff').buffers()` to continue
working, add a tiny shim in your config:

```lua
local plus = require('fff_plus')
local fff = require('fff')

fff.buffers = plus.buffers
fff.colors = plus.colors
fff.git_files = plus.git_files
fff.git_status = plus.git_status
fff.tracked_files = plus.tracked_files
```

## Maintenance Notes

This repo intentionally does not carry upstream backend code. That keeps the
sync burden low, but these pickers still reuse upstream Lua internals:

- `fff.conf`
- `fff.file_picker.preview`
- `fff.file_picker.icons`
- `fff.utils`
- `fff.highlights`

If upstream refactors those modules, `fff-plus.nvim` may need a compatibility
update. The long-term ideal is a small public picker-extension API in upstream
`fff.nvim`.

See [EXPERIMENT_EXTENSION_PLUGIN.md](./EXPERIMENT_EXTENSION_PLUGIN.md) for the
full tradeoff notes.
