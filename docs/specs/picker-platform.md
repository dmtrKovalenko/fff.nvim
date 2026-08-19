# Spec: Shared Picker Platform

## Objective

Turn fff-plus.nvim's bespoke pickers into adapters over one shared picker
module, then use that module to add high-value editor workflows without
duplicating picker UI code.

The change must preserve the existing buffers, colors, tracked-files, and
Git-status interfaces while adding:

- asynchronous, cancellable Git sources and previews;
- shared actions, help, resume, query history, and responsive layouts;
- smart-case matching, query operators, field filters, and match positions;
- Git stage, unstage, restore, and refresh actions;
- smart, lines, and diagnostics pickers;
- an integration smoke test against real upstream fff.

The later symbols, marks, jumps, command/search-history, quickfix/location-list
sources and the optional `vim.ui.select` adapter are not part of this change.

## Tech Stack

- LuaJIT-compatible Lua
- Neovim Lua interfaces, including `vim.system`
- Upstream `dmtrKovalenko/fff`
- Headless Neovim tests
- Stylua formatting

No new runtime dependency is allowed.

## Commands

- Full verification: `make all`
- Local tests: `make test`
- Real-upstream smoke test: `make test-integration FFF_UPSTREAM=/path/to/fff`
- Lua bytecode check: `make lint`
- Formatting check: `make format-check`
- Apply formatting: `make format`

## Project Structure

- `lua/fff_plus/picker.lua`: shared picker module and its public interface
- `lua/fff_plus/sources/`: source adapters and source-specific operations
- `lua/fff_plus/pickers/`: compatibility entrypoints for existing callers
- `lua/fff_plus/matcher.lua`: query parsing, scoring, and match positions
- `lua/fff_plus/process.lua`: cancellable process adapter
- `tests/`: small behavior tests and real-upstream smoke test
- `docs/specs/`: accepted implementation specifications

## Code Style

Adapters describe source-specific behavior; the shared module owns UI state and
lifecycle:

```lua
return {
  name = 'lines',
  items = function(ctx) return collect_lines(ctx.opts) end,
  text = function(item) return item.text end,
  format = function(item) return item.display end,
  confirm = function(ctx, item) ctx:jump(item) end,
}
```

Use small functions, early returns, `vim.validate` at public seams, and Stylua's
existing formatting. Process arguments remain arrays and never interpolate user
input into shell command strings.

## Testing Strategy

- Write a failing behavior test before each implementation slice.
- Test matcher, history, source transforms, selection, and process result
  handling as small deterministic tests.
- Test the shared picker through its interface with in-memory source adapters.
- Keep one headless UI smoke path for compatibility entrypoints.
- Run a separate integration smoke test with real upstream modules rather than
  replacing them through `package.loaded` stubs.
- Run `make all` after each completed slice.

## Boundaries

- Always: preserve existing commands, functions, key defaults, and cancellation
  behavior; confirm destructive restore actions; cancel stale jobs; test each
  behavior before implementation.
- Allowed by this specification: add the real-upstream integration target and
  continuous-integration coverage needed to run it.
- Never: add runtime dependencies, add modules under `lua/fff/`, vendor upstream
  code, add backend/binary/release code, or execute destructive Git actions
  without explicit in-editor confirmation.

## Success Criteria

1. Existing buffers, colors, tracked-files, and Git-status entrypoints use the
   shared picker module and retain their documented actions.
2. A source adapter does not create windows, manage picker buffers, or own
   viewport and query state.
3. Git listing and preview commands do not block Neovim and stale callbacks
   cannot overwrite current results.
4. Matching supports smart-case, exact/prefix/suffix/exclusion terms, source
   field filters, and matched-character positions.
5. Help, resume, history navigation, preview toggle, maximize, list/preview
   focus, select-all, and location-list actions work through the shared module.
6. Git status supports stage, unstage, restore with confirmation, and refresh.
7. Smart results combine buffers, old files, and upstream indexed files,
   normalize and deduplicate paths, and retain the best ranking metadata.
8. Lines supports current-buffer and loaded-buffer scope; diagnostics supports
   current-buffer and workspace scope.
9. README installation uses `dmtrKovalenko/fff` and documents new pickers and
   actions.
10. `make all` and the real-upstream integration smoke test pass.

## Implementation Tasks

- [ ] Add integration smoke coverage and update the upstream installation name.
- [ ] Define and test the shared picker interface, then migrate one existing
      picker without behavior loss.
- [ ] Migrate the remaining existing pickers and delete duplicated lifecycle
      implementations.
- [ ] Add the process adapter and move Git listing and preview work off the main
      loop.
- [ ] Add shared actions, help, resume/history, and responsive layouts.
- [ ] Extend matching and rendered match highlighting.
- [ ] Add safe Git status mutation actions.
- [ ] Add smart, lines, and diagnostics source adapters and public entrypoints.
- [ ] Update documentation and run full verification.

## Open Questions

None. The accepted scope preserves compatibility and defers the explicitly
later source catalog and optional `vim.ui.select` adapter.
