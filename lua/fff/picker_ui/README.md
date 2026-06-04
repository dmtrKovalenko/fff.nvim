# `fff.picker_ui` — Picker UI Module

This directory contains the full implementation of the FFF file picker UI.
Originally a single monolithic ~2535-line file, it has been split into focused submodules.

## Entry point

**`picker_ui.lua`** — requires `require('fff.picker_ui.picker_ui')`.

This is the coordinator. It wires all submodules together and exposes the public API:

| Function | Purpose |
|---|---|
| `M.open(opts)` | Open the file picker |
| `M.open_with_callback(query, callback, opts)` | Search with a callback, fall back to opening the picker |
| `M.select(action)` | Open the selected file (edit/split/vsplit/tab) |
| `M.toggle_select()` | Toggle multi-selection for the current item |
| `M.send_to_quickfix()` | Send selected items to the quickfix list |
| `M.toggle_debug()` | Toggle debug score display |
| `M.monitor_scan_progress()` | Poll indexing progress during initial scan |

## Submodules

| Module | File | Purpose | Key exports via `picker_ui.lua` |
|---|---|---|---|
| **picker_ui_state** | `picker_ui_state.lua` | Single source of truth for all picker state | `M.state`, `M.clear_selections`, `M.reset_history_state` |
| **ui_creator** | `ui_creator.lua` | Creates buffers, windows, and keymaps for the picker UI | `M.create_ui`, `M.setup_buffers`, `M.setup_windows`, `M.setup_keymaps`, `M.focus_*_win`, `M.open_preview`, `M.close_preview` |
| **search_manager** | `search_manager.lua` | Executes searches, manages pagination, handles query history | `M.update_results_sync`, `M.update_results`, `M.load_*_page`, `M.on_input_change`, `M.cycle_grep_modes`, `M.recall_query_from_history`, `M.cycle_forward_query`, `M.get_suggestion_renderer` |
| **renderer** | `renderer.lua` | Renders the file list, handles combo separator, scrollbar, and empty state | `M.render_list`, `M.scroll_to_bottom` |
| **preview_manager** | `preview_manager.lua` | Manages file preview rendering, debounced updates, and preview title | `M.update_preview`, `M.update_preview_smart`, `M.update_preview_debounced`, `M.update_preview_title`, `M.clear_preview` |
| **navigation** | `navigation.lua` | Handles cursor movement, pagination wrap-around, and preview scrolling | `M.move_up`, `M.move_down`, `M.wrap_to_first`, `M.wrap_to_last`, `M.scroll_preview_up`, `M.scroll_preview_down` |
| **layout_manager** | `layout_manager.lua` | Recalculates layout on terminal resize (VimResized) and cleans up on close | `M.relayout`, `M.close` |
| **file_renderer** | `file_renderer.lua` | Renders individual file lines with icon, path, and score | `M.render_line`, `M.apply_highlights` |
| **grep_renderer** | `grep_renderer.lua` | Grep search execution and rendering of grep result lines | `M.search`, `M.get_search_metadata`, `M.render_line`, `M.apply_highlights` |
| **combo_renderer** | `combo_renderer.lua` | Detects combo (directory common prefix) items from a list | `M.detect` |
| **list_renderer** | `list_renderer.lua` | Renders the full file list with separator, scrollbar, and combo display | `M.render` |
| **utils** | `utils.lua` | Utility functions (quickfix list building) | `M.send_to_quickfix` |

### Module relationships

```
picker_ui.lua (coordinator)
├── ui_creator.lua    — buffer/window/keymap creation
├── search_manager.lua → grep_renderer.lua → file_renderer.lua
├── renderer.lua      → list_renderer.lua, combo_renderer.lua
│                        ├─ file_renderer.lua
│                        └─ list_separator (external)
├── preview_manager.lua
├── navigation.lua
├── layout_manager.lua
└── utils.lua         → grep_renderer.lua (for exhaustive grep in send_to_quickfix)
```

## Architecture

All submodules follow the same pattern:

```lua
local M = {}
local P = nil  -- parent module reference

function M.init(parent_module) P = parent_module end

local S = picker_ui_state.state  -- shared state

-- ... functions that use S.* and P.* ...

return M
```

The coordinator (`picker_ui.lua`) calls `module.init(M)` on each submodule, passing itself as the parent. This lets submodules call back into `picker_ui.lua` for cross-module coordination — for example, `navigation.lua` calls `P.render_list()` and `P.update_preview()` after moving the cursor.

State is shared via a single table reference (`picker_ui_state.state`). Every submodule writes to and reads from the same table — no message passing or event bus.

Modules without `init()` are either pure data stores (`picker_ui_state`) or standalone utility modules without cross-module dependencies (`file_renderer`, `grep_renderer`, `combo_renderer`, `list_renderer`, `utils`).

## Key design decisions

- **`picker_ui_state` is a pure data store** — no parent module reference, no `init()`. It owns the state table and selection helpers.
- **`picker_ui.lua` keeps cross-cutting concerns** — `update_status`, `select`, `toggle_debug`, `open` lives here because they coordinate across multiple submodules.
- **Module purpose is reflected in the filename** — anything with `_manager` in the name manages state or lifecycle; `renderer` and `navigation` are purely behavioral; `*_renderer` modules handle rendering of specific item types.
- **`vim.schedule` usage is intentional** — deferred calls prevent re-entrancy issues during buffer mutations and window teardown.
