# Fork Maintenance

This repository is maintained as `fff-plus.nvim`, not as a patch queue waiting
for upstream acceptance. It tracks upstream where useful, but it prioritizes a
fuller Neovim picker ecosystem.

## Policy

- Keep upstream-derived core search code close to upstream when practical.
- Put fork-owned picker behavior behind `lua/fff/pickers`.
- Route public picker entrypoints in `lua/fff/main.lua` through the picker
  registry instead of requiring picker modules directly.
- Prefer small upstream sync branches over long-running rebases.
- Do not depend on upstream adding extension points before shipping useful
  picker features here.

## Picker Boundary

Fork-owned pickers live under `lua/fff/pickers`, expose an `open(opts)`
function, and are registered in `lua/fff/pickers/init.lua`.

```lua
require('fff.pickers').open('buffers', opts)
```

The registry is the stable internal boundary for picker entrypoints. If the UI
implementation changes later, public APIs such as `require('fff').buffers()`
should not need to change.

Keep old top-level picker module paths as compatibility shims when moving an
existing picker. For example, `lua/fff/buffers.lua` should continue to return
`require('fff.pickers.buffers')`.

## Upstream Syncs

Use short-lived branches for upstream imports:

```bash
git fetch upstream
git switch -c sync/upstream-YYYY-MM-DD
git merge upstream/main
```

Resolve conflicts once on the sync branch, run the relevant checks, then merge
back into the active fork branch.
