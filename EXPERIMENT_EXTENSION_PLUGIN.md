# Experiment: fff-plus.nvim as an upstream extension

This branch explores a lower-maintenance shape for `fff-plus.nvim`: instead of
forking `fff.nvim` and carrying upstream sync work, `fff-plus.nvim` can become a
separate Neovim plugin that depends on upstream `fff.nvim`.

## Proposed user install

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
    { '<leader>g', function() require('fff_plus').git_files() end, desc = 'FFF+ git files' },
    { '<leader>c', function() require('fff_plus').colors() end, desc = 'FFF+ colors' },
  },
}
```

The extension owns `require('fff_plus')`, `:FFFPlusBuffers`,
`:FFFPlusColors`, and `:FFFPlusGFiles`. It can optionally provide compatibility
commands with `legacy_commands = true`.

## Pros

- Much lower upstream-sync burden. Upstream owns the Rust binary, downloader,
  core picker, file search, live grep, and releases.
- Cleaner social boundary. Your plugin is an addon, not a competing fork of the
  whole project.
- Faster iteration on picker UX. Buffers, git-status files, and colorschemes can
  evolve independently.
- Users can update upstream `fff.nvim` normally and keep your extras layered on
  top.
- Smaller release surface for you. You do not need to publish prebuilt Rust
  binaries if upstream remains responsible for them.

## Cons

- You depend on upstream internals. These pickers currently reuse modules such
  as `fff.conf`, `fff.file_picker.preview`, `fff.file_picker.icons`, and
  `fff.highlights`, which are not a formal extension API.
- Breakage may be subtler. A harmless upstream refactor could break
  `fff-plus.nvim` even if upstream itself is healthy.
- You lose control over binary release cadence, downloader behavior, and any
  upstream defaults you care about.
- Users now install two plugins, and load order matters.
- If upstream stays hostile or changes internals deliberately, this shape avoids
  merge conflicts but not compatibility work.

## Experiment result

This branch adds a separate extension namespace:

- `lua/fff_plus/init.lua`
- `lua/fff_plus/pickers/buffers.lua`
- `lua/fff_plus/pickers/colors.lua`
- `lua/fff_plus/pickers/git_files.lua`
- `lua/fff_plus/git_utils.lua`
- `plugin/fff_plus.lua`

The important architectural idea is that future public APIs should live under
`fff_plus`, not under `fff`.

Before making this the real plugin, the main follow-up should be to reduce the
dependency on upstream internals by asking for or designing a tiny upstream
picker-extension API.
