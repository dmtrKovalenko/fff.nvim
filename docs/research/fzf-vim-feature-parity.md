# fzf.vim feature-parity research

Checked 2026-07-20. The primary external baseline is
[`junegunn/fzf.vim` at `d2a59a9`](https://github.com/junegunn/fzf.vim/tree/d2a59a992a2455f609c0fde2ebd84427ea8f919a).
The local side is the combined product: upstream
[`dmtrKovalenko/fff` at `073698c`](https://github.com/dmtrKovalenko/fff/tree/073698c8e7dad9f6106d23ef588f5ebc7f98b326)
plus this `fff-plus.nvim` checkout.

> Implementation update: [issue #6](https://github.com/vinitkumar/fff-plus.nvim/issues/6)
> resolves the audit's extension-picker gaps
> for fuzzy matching, long-list scrolling, tracked/status Git separation,
> multi-select and quickfix, paste actions, fullscreen commands, existing-window
> buffer jumps, and Git diff previews. The command table below records the
> pre-implementation baseline that motivated that work.

## Target resolution

The request says “fzf.nvim”, but the intended target in this repository is
`junegunn/fzf.vim`:

- The local README calls the aliases “fzf.vim-style workflows”
  ([`README.md`, lines 75–90](../../README.md#L75-L90)).
- The three extension pickers identify themselves with fzf.vim's `:Buffers`,
  `:Colors`, and `:GFiles?` commands
  ([buffers](../../lua/fff_plus/pickers/buffers.lua#L1),
  [colors](../../lua/fff_plus/pickers/colors.lua#L1),
  [git status](../../lua/fff_plus/pickers/git_files.lua#L1)).
- Those exact commands appear in the
  [official fzf.vim command table](https://github.com/junegunn/fzf.vim/blob/d2a59a992a2455f609c0fde2ebd84427ea8f919a/README.md#commands).

This is not `ibhagwan/fzf-lua`, which is a separate and much broader Lua picker
suite with buffers/files, search, tags, Git, LSP/diagnostics, DAP, editor-state,
completion, resume, combine, and global-picker features
([official command list](https://github.com/ibhagwan/fzf-lua#commands)). It is
also not the literal-name [`samsze0/fzf.nvim`](https://github.com/samsze0/fzf.nvim),
whose README does not enumerate a stable implemented picker surface. If the
literal project was intended, a separate source-level audit is needed.

## Status definitions

| Status | Meaning |
| --- | --- |
| **Full** | The combined FFF setup provides the same user workflow and its important actions. |
| **Partial** | The core workflow exists, but search behavior, scope, or important actions differ. |
| **Alternative** | FFF reaches a similar outcome through a materially different workflow or engine. |
| **Missing** | No equivalent user-facing picker or action was found. |

The status is about user workflows, not implementation parity with the `fzf`
binary.

## Command-by-command parity

The benchmark definitions in this table come from fzf.vim's
[official command list](https://github.com/junegunn/fzf.vim/blob/d2a59a992a2455f609c0fde2ebd84427ea8f919a/README.md#commands).

| fzf.vim workflow | Combined FFF equivalent | Status | Evidence / gap |
| --- | --- | --- | --- |
| `:Files [PATH]` | `fff.find_files()` / `find_files_in_dir(path)` | **Full** | Upstream exposes both current-repository and specific-directory file pickers ([public API](https://github.com/dmtrKovalenko/fff/blob/073698c8e7dad9f6106d23ef588f5ebc7f98b326/README.md#public-api)). |
| `:GFiles [OPTS]` — tracked files from `git ls-files` | None | **Missing** | The local `:GFiles` alias is misleading: it calls `git_files()`, which reads `git status -s`, not `git ls-files` ([command registration](../../lua/fff_plus/init.lua#L126-L129), [source](../../lua/fff_plus/pickers/git_files.lua#L26-L30)). Upstream file search is Git-aware but is not a tracked-files-only picker. |
| `:GFiles?` — Git status files | `fff_plus.git_files()` / `:FFFPlusGFiles` | **Partial** | It reads and labels `git status -s` entries and supports edit/split/vsplit/tab ([source](../../lua/fff_plus/pickers/git_files.lua#L26-L98), [actions](../../lua/fff_plus/pickers/git_files.lua#L514-L536)). Unlike fzf, matching is literal case-insensitive substring, not fuzzy ([filter](../../lua/fff_plus/pickers/git_files.lua#L403-L415)); it previews file contents rather than the status diff and has no multi-select, quickfix, or paste action. fzf.vim documents `delta`-formatted diff previews for this workflow ([dependencies](https://github.com/junegunn/fzf.vim/blob/d2a59a992a2455f609c0fde2ebd84427ea8f919a/README.md#dependencies)). |
| `:Buffers` | `fff_plus.buffers()` / `:FFFPlusBuffers` | **Partial** | It lists MRU-sorted listed buffers, previews them, opens in edit/split/vsplit/tab, and can delete a buffer ([collection](../../lua/fff_plus/pickers/buffers.lua#L37-L59), [actions](../../lua/fff_plus/pickers/buffers.lua#L655-L710)). Matching is literal substring despite the “fuzzy” comment ([filter](../../lua/fff_plus/pickers/buffers.lua#L138-L153)); there is no multi-select or paste action. |
| `:Colors` | `fff_plus.colors()` / `:FFFPlusColors` | **Partial** | It live-previews schemes and restores the original on cancel ([preview/select](../../lua/fff_plus/pickers/colors.lua#L424-L496)). Matching is literal substring ([filter](../../lua/fff_plus/pickers/colors.lua#L94-L109)). `:Colors!` accepts a bang, but the documented fullscreen/no-preview behavior is not implemented: `opts.bang` is only merged into config ([registration](../../lua/fff_plus/init.lua#L87-L94), [open](../../lua/fff_plus/pickers/colors.lua#L513-L535)). |
| `:Ag [PATTERN]` | `fff.live_grep({ query = ... })` | **Alternative** | FFF provides indexed plain/regex/fuzzy content search rather than an `ag` result picker ([live grep modes](https://github.com/dmtrKovalenko/fff/blob/073698c8e7dad9f6106d23ef588f5ebc7f98b326/README.md#live-grep-modes)). No `ag` backend compatibility. |
| `:Rg [PATTERN]` — search then fuzzy-filter results | `fff.live_grep({ query = ... })` | **Alternative** | FFF searches its content index as the query changes; it does not expose fzf.vim's fixed ripgrep-result source followed by a separate fuzzy filter. It does provide query prefill and plain/regex/fuzzy modes ([live grep modes](https://github.com/dmtrKovalenko/fff/blob/073698c8e7dad9f6106d23ef588f5ebc7f98b326/README.md#live-grep-modes)). |
| `:RG [PATTERN]` — relaunch search on every keystroke | `fff.live_grep()` | **Alternative** | Same interactive project-content-search outcome, but backed by FFF's in-memory index rather than relaunching `rg` ([public API](https://github.com/dmtrKovalenko/fff/blob/073698c8e7dad9f6106d23ef588f5ebc7f98b326/README.md#public-api)). |
| `:Lines [QUERY]` — lines in loaded buffers | None | **Missing** | Project-wide live grep is not a loaded-buffers-only line picker. |
| `:BLines [QUERY]` — current-buffer lines | None | **Missing** | No current-buffer line picker was found. |
| `:Tags [PREFIX]` | None | **Missing** | No project tags picker or ctags integration was found. |
| `:BTags [QUERY]` | None | **Missing** | No current-buffer tags picker was found. |
| `:Changes` | None | **Missing** | No changelist picker was found. |
| `:Marks` / `:BMarks` | None | **Missing** | No global/current-buffer marks picker was found. |
| `:Jumps` | None | **Missing** | No jumplist picker was found. |
| `:Windows` | None | **Missing** | No window picker was found. Buffer selection is not window selection. |
| `:Locate PATTERN` | None | **Missing** | No picker over system `locate` output was found. |
| `:History` — old files and open buffers | Buffer picker only | **Partial** | The extension covers open buffers, but there is no `v:oldfiles`/recent-files picker ([documented pickers](../../README.md#L67-L73)). FFF's query-history scoring is not a recent-files picker. |
| `:History:` — command history | None | **Missing** | No command-history picker was found. |
| `:History/` — search history | None | **Missing** | FFF persists picker queries for ranking, but does not expose a search-history picker ([history configuration](https://github.com/dmtrKovalenko/fff/blob/073698c8e7dad9f6106d23ef588f5ebc7f98b326/README.md#configuration)). |
| `:Snippets` | None | **Missing** | No UltiSnips/snippet picker was found. |
| `:Commits [LOG_OPTS]` | None | **Missing** | Git-status files are not project commit history. |
| `:BCommits [LOG_OPTS]` | None | **Missing** | No current-buffer commit-history picker was found. |
| `:Commands` | None | **Missing** | No Ex-command picker was found. |
| `:Maps` | None | **Missing** | No normal-mode mapping picker was found. |
| `:Helptags` | None | **Missing** | No help-tag picker was found. |
| `:Filetypes` | None | **Missing** | No filetype picker was found. |

## Cross-cutting UX and extensibility parity

fzf.vim documents these behaviors alongside its commands and in its
[customization](https://github.com/junegunn/fzf.vim/blob/d2a59a992a2455f609c0fde2ebd84427ea8f919a/README.md#customization),
[advanced customization](https://github.com/junegunn/fzf.vim/blob/d2a59a992a2455f609c0fde2ebd84427ea8f919a/README.md#advanced-customization),
and [completion](https://github.com/junegunn/fzf.vim/blob/d2a59a992a2455f609c0fde2ebd84427ea8f919a/README.md#completion-functions)
sections.

| Capability | Combined FFF status | Evidence / gap |
| --- | --- | --- |
| Open in tab, horizontal split, or vertical split | **Full for file-like pickers** | Upstream binds select/split/vsplit/tab and the buffer/Git-status extension pickers implement the same four actions ([upstream keymaps](https://github.com/dmtrKovalenko/fff/blob/073698c8e7dad9f6106d23ef588f5ebc7f98b326/README.md#configuration), [extension Git actions](../../lua/fff_plus/pickers/git_files.lua#L514-L536)). |
| Preview pane | **Partial / alternative** | Upstream FFF has configurable preview placement and size; buffer and Git-status pickers reuse it, while Colors performs a live colorscheme preview ([upstream preview config](https://github.com/dmtrKovalenko/fff/blob/073698c8e7dad9f6106d23ef588f5ebc7f98b326/README.md#configuration), [local dependency](../../README.md#L104-L117)). The extension has no runtime preview toggle, and its Git-status picker shows file contents rather than a diff. |
| Multi-select and send results to quickfix/location list | **Partial** | Upstream file/grep pickers provide toggle-select and send-to-quickfix keymaps ([configuration](https://github.com/dmtrKovalenko/fff/blob/073698c8e7dad9f6106d23ef588f5ebc7f98b326/README.md#configuration)). Extension pickers have only single-item selection; there is no location-list target. |
| Paste selected item into the current buffer | **Missing** | No counterpart to fzf.vim's configurable `ALT-ENTER` paste action was found. |
| Bang command opens fullscreen | **Missing** | `FFFPlusGFiles` registers `bang = true` but discards the command callback options; Colors passes `bang` without acting on it ([commands](../../lua/fff_plus/init.lua#L87-L99), [Colors open](../../lua/fff_plus/pickers/colors.lua#L513-L535)). |
| True fuzzy matching in every picker | **Partial** | Upstream file/grep use FFF's typo-tolerant search, but all three extension pickers use `string.find(..., true)`, i.e. case-insensitive literal substring matching ([buffers](../../lua/fff_plus/pickers/buffers.lua#L142-L150), [colors](../../lua/fff_plus/pickers/colors.lua#L98-L106), [Git status](../../lua/fff_plus/pickers/git_files.lua#L403-L414)). |
| Jump to the window already showing a selected buffer | **Missing** | Buffer selection executes `:buffer` in the current window; there is no equivalent to fzf.vim's `g:fzf_vim.buffers_jump` ([selection](../../lua/fff_plus/pickers/buffers.lua#L655-L678)). |
| Per-command options | **Alternative / partial** | FFF and FFF+ accept Lua option tables and merge picker config, but do not expose arbitrary per-command fzf CLI options ([FFF public API](https://github.com/dmtrKovalenko/fff/blob/073698c8e7dad9f6106d23ef588f5ebc7f98b326/README.md#public-api), [FFF+ dispatch](../../lua/fff_plus/init.lua#L37-L70)). |
| Configurable command prefix | **Missing** | FFF+ offers only fixed optional legacy aliases, not a general prefix ([configuration and aliases](../../lua/fff_plus/init.lua#L9-L11), [registration](../../lua/fff_plus/init.lua#L101-L129)). |
| Public API for custom picker sources/sinks | **Partial** | Upstream has programmatic `file_search` and `content_search`, but FFF+ exposes a fixed three-picker registry. There is no public equivalent of `fzf#wrap`, arbitrary source/sink specs, or `fzf#vim#with_preview` ([upstream programmatic API](https://github.com/dmtrKovalenko/fff/blob/073698c8e7dad9f6106d23ef588f5ebc7f98b326/README.md#public-api), [local registry](../../lua/fff_plus/init.lua#L3-L7)). |
| Insert-mode word/path/file/line completion | **Missing** | No fuzzy completion functions or `<Plug>` completion mappings were found. |
| Mapping selector across normal/insert/visual/operator modes | **Missing** | No equivalent to fzf.vim's mapping-selector `<Plug>` mappings was found. |
| Long-list viewport scrolling in extension pickers | **Missing** | The extension renderers always draw the first `win_height` items, while movement can advance the logical cursor past that slice; there is no scroll offset ([buffer render](../../lua/fff_plus/pickers/buffers.lua#L428-L458), [buffer movement](../../lua/fff_plus/pickers/buffers.lua#L625-L641), [Git-status render](../../lua/fff_plus/pickers/git_files.lua#L302-L332)). Large buffer and Git-status lists can therefore select or preview an off-screen item without visibly rendering it. |

## What is materially missing

The largest parity gaps are not more file search. They are:

1. **Editor-state pickers:** buffer/current-buffer lines, changes, marks, jumps,
   windows, old files, command/search history, commands, mappings, help tags,
   and filetypes.
2. **Code and Git navigation:** project/buffer tags, tracked-only Git files,
   project commits, and per-buffer commits.
3. **Shared picker behavior:** true fuzzy matching in extension pickers,
   extension-wide multi-select/quickfix, paste action, functional bang/fullscreen,
   existing-window buffer jumps, diff previews for Git status, and correct
   viewport scrolling for long lists.
4. **Extension surface:** arbitrary/custom picker sources, sinks/actions, preview
   helpers, and insert-mode completion.

[Issue #6](https://github.com/vinitkumar/fff-plus.nvim/issues/6) corrects the
`:GFiles` compatibility alias to use tracked files from `git ls-files` and adds
explicit Git-files and Git-status APIs.

## Sources

- [fzf.vim README at audited revision](https://github.com/junegunn/fzf.vim/blob/d2a59a992a2455f609c0fde2ebd84427ea8f919a/README.md)
- [upstream FFF README at audited revision](https://github.com/dmtrKovalenko/fff/blob/073698c8e7dad9f6106d23ef588f5ebc7f98b326/README.md)
- [fzf-lua official README](https://github.com/ibhagwan/fzf-lua)
- [literal-name samsze0/fzf.nvim README](https://github.com/samsze0/fzf.nvim)
- Local files linked inline above
