# Repository Notes

This branch is an experiment that turns `fff-plus.nvim` into a small extension
plugin for upstream `fff.nvim`.

## Scope

- Keep extension-owned code under `lua/fff_plus/`.
- Keep the runtime loader in `plugin/fff_plus.lua`.
- Do not add `lua/fff/` modules here; upstream `fff.nvim` owns that namespace.
- Do not add Rust crates, binary downloaders, package SDKs, MCP code, or release
  workflows to this branch.

## Commands

- `make test` runs the headless Neovim extension test.
- `make lint` bytecode-checks the Lua files with LuaJIT.
- `make format-check` verifies Stylua formatting.
- `make format` applies Stylua formatting.

## Design Notes

The extension currently depends on upstream internal Lua modules:

- `fff.conf`
- `fff.file_picker.preview`
- `fff.file_picker.icons`
- `fff.utils`
- `fff.highlights`

Treat those as compatibility risks when changing picker code.
