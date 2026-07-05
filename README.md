<img alt="f3+ logo for fff-plus.nvim" src="./assets/logo-f3-plus-orange.png" width="300">

# fff-plus.nvim

Extra pickers for [fff.nvim](https://github.com/dmtrKovalenko/fff.nvim).

`fff-plus.nvim` is an extension plugin. It does not ship the Rust backend,
binary downloader, file picker, live grep, frecency index, or release pipeline.
Those stay owned by upstream `fff.nvim`. This plugin layers on the missing
Neovim picker surface:

- `:FFFPlusBuffers`
- `:FFFPlusColors`
- `:FFFPlusGFiles`
- `require('fff_plus').buffers()`
- `require('fff_plus').colors()`
- `require('fff_plus').git_files()`

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
},
{
  'vinitkumar/fff-plus.nvim',
  dependencies = { 'dmtrKovalenko/fff.nvim' },
  opts = {
    legacy_commands = false,
  },
  keys = {
    { '<leader>b', function() require('fff_plus').buffers() end, desc = 'FFF+ buffers' },
    { '<leader>c', function() require('fff_plus').colors() end, desc = 'FFF+ colors' },
    { '<leader>g', function() require('fff_plus').git_files() end, desc = 'FFF+ git files' },
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

vim.g.fff_plus = {
  legacy_commands = false,
}

vim.keymap.set('n', '<leader>b', function() require('fff_plus').buffers() end, { desc = 'FFF+ buffers' })
vim.keymap.set('n', '<leader>c', function() require('fff_plus').colors() end, { desc = 'FFF+ colors' })
vim.keymap.set('n', '<leader>g', function() require('fff_plus').git_files() end, { desc = 'FFF+ git files' })
```

## Commands

| Command | Description |
| --- | --- |
| `:FFFPlusBuffers` | Switch between listed buffers with preview and delete support |
| `:FFFPlusColors` | Browse and apply colorschemes with live preview |
| `:FFFPlusGFiles` | Browse files from `git status -s` with status indicators |

Set `legacy_commands = true` to also register `:FFFBuffers`, `:Colors`, and
`:GFiles` as compatibility aliases.

## Maintenance Shape

This branch is an experiment in reducing fork maintenance. The upside is much
less upstream sync work: upstream owns the heavy backend and release machinery.
The main risk is that these pickers still reuse upstream Lua internals such as
`fff.conf`, `fff.file_picker.preview`, `fff.file_picker.icons`, `fff.utils`, and
`fff.highlights`. A future upstream refactor can still require compatibility
work here.

See [EXPERIMENT_EXTENSION_PLUGIN.md](./EXPERIMENT_EXTENSION_PLUGIN.md) for the
pros, cons, and design notes.
