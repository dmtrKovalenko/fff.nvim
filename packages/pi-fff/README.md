# @ff-labs/pi-fff

A [pi](https://github.com/badlogic/pi-mono) extension that replaces the built-in `find` and `grep` tools with [FFF](https://github.com/dmtrKovalenko/fff.nvim) — a Rust-native, SIMD-accelerated file finder with built-in memory.

## What it does

| Built-in tool | pi-fff replacement | Improvement |
|---|---|---|
| `find` (spawns `fd`) | `fffind` (FFF `fileSearch`) | Fuzzy matching, frecency ranking, git-aware, pre-indexed |
| `grep` (spawns `rg`) | `ffgrep` (FFF `grep`) | SIMD-accelerated, frecency-ordered, mmap-cached, no subprocess |
| *(none)* | `fff-multi-grep` (FFF `multiGrep`) | OR-logic multi-pattern search via Aho-Corasick |
| `@` file autocomplete (fd-backed) | `@` file autocomplete (FFF-backed, default) | Fuzzy ranking from FFF index/frecency |

### Key advantages over built-in tools

- **No subprocess spawning** — FFF is a Rust native library called through the Node binding. No `fd`/`rg` process per call.
- **Pre-indexed** — files are indexed in the background at session start. Searches are instant.
- **Frecency ranking** — files you access often rank higher. Learns across sessions.
- **Query history** — remembers which files were selected for which queries. Combo boost.
- **Git-aware** — modified/staged/untracked files are boosted in results.
- **Smart case** — case-insensitive when query is all lowercase, case-sensitive otherwise.
- **Fuzzy file search** — `find` uses fuzzy matching, not glob-only. Typo-tolerant.
- **Cursor pagination** — grep results include a cursor for fetching the next page.

## Install

Requirements:
- pi

### Install as a pi package

**Via npm (recommended):**

```bash
pi install npm:@ff-labs/pi-fff
```

Project-local install:

```bash
pi install -l npm:@ff-labs/pi-fff
```

**Via git:**

```bash
pi install git:github.com/dmtrKovalenko/fff.nvim
```

Pin to a release:

```bash
pi install git:github.com/dmtrKovalenko/fff.nvim@v0.3.0
```

### Local development / manual install

```bash
git clone https://github.com/dmtrKovalenko/fff.nvim.git
cd fff.nvim/packages/pi-fff
npm install
```

Then add to your pi `settings.json`:

```json
{
  "extensions": ["/path/to/fff.nvim/packages/pi-fff/src/index.ts"]
}
```

Or test directly:

```bash
pi -e /path/to/fff.nvim/packages/pi-fff/src/index.ts
```

This extension registers FFF-powered tools (`fffind`, `ffgrep`, `fff-multi-grep`) alongside pi's built-in tools.

## Tools

### `ffgrep`

Search file contents. Smart case, plain text by default, regex optional.

Parameters:
- `pattern` — search text or regex
- `path` — directory/file constraint (e.g. `src/`, `*.ts`)
- `ignoreCase` — force case-insensitive
- `literal` — treat as literal string (default: true)
- `context` — context lines around matches
- `limit` — max matches (default: 100)
- `cursor` — pagination cursor from previous result

### `fffind`

Fuzzy file name search. Frecency-ranked.

Parameters:
- `pattern` — fuzzy query (e.g. `main.ts`, `src/ config`)
- `path` — directory constraint
- `limit` — max results (default: 200)

### `fff-multi-grep`

OR-logic multi-pattern content search. SIMD-accelerated Aho-Corasick.

Parameters:
- `patterns` — array of literal patterns (OR logic)
- `constraints` — file constraints (e.g. `*.{ts,tsx} !test/`)
- `context` — context lines
- `limit` — max matches (default: 100)
- `cursor` — pagination cursor

## Commands

- `/fff-health` — show FFF status (indexed files, git info, frecency/history DB status)
- `/fff-rescan` — trigger a file rescan
- `/fff-mode <mode>` — switch mode (tool name change requires restart)

## Modes

- `tools-and-ui` (default): registers `fffind`, `ffgrep`, `fff-multi-grep` as additional tools + FFF-backed `@` autocomplete
- `tools-only`: additional tools only; keep pi's default `@` autocomplete
- `override`: replaces pi's built-in `find`, `grep` and adds `multi_grep` + FFF-backed `@` autocomplete

Mode precedence:
1. `--fff-mode <mode>` CLI flag
2. `PI_FFF_MODE=<mode>` environment variable
3. default (`tools-and-ui`)

## Flags

- `--fff-mode <mode>` — set mode (see above)
- `--fff-frecency-db <path>` — path to frecency database (also: `FFF_FRECENCY_DB` env)
- `--fff-history-db <path>` — path to query history database (also: `FFF_HISTORY_DB` env)

## Data

When database paths are provided, FFF stores:
- frecency database — file access frequency/recency
- history database — query-to-file selection history

No project files are uploaded anywhere by this extension. It runs locally and only uses the configured LLM through pi itself.

## Security

- No shell execution
- No network calls in the extension code
- No telemetry
- No credential handling beyond whatever pi and your configured model provider already do
- Search state is stored locally under `~/.pi/agent/fff/`
