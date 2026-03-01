# FFF MCP Server

**Drop-in replacement for AI code assistant file search tools**

FFF MCP (Model Context Protocol) server provides intelligent file finding and code search with frecency ranking, fuzzy matching, and git-aware scoring. It's a production-ready alternative to Claude Code's native Glob/Grep tools, designed to help AI assistants find code faster with fewer attempts and lower token consumption.

## Key Features

- **Frecency Ranking**: Surfaces frequently-accessed and recently-modified files first
- **Fuzzy Matching**: Handles typos and approximate searches automatically
- **Git-Aware Scoring**: Boosts modified/dirty files in results
- **SIMD-Accelerated Grep**: Fast content search powered by Rust
- **Workspace Detection**: Automatically detects monorepo boundaries (npm, Cargo, Go)
- **Compact Output**: Token-efficient result formatting
- **Zero Configuration**: Works out of the box with sensible defaults

## Installation

```bash
npm install -g @ff-labs/fff-mcp
```

Or use directly with `npx`:

```bash
npx @ff-labs/fff-mcp
```

## Setup

### Claude Code

Add to your Claude Code configuration file:

**macOS/Linux**: `~/.config/claude/mcp.json`
**Windows**: `%APPDATA%\Claude\mcp.json`

```json
{
  "mcpServers": {
    "fff": {
      "command": "npx",
      "args": ["-y", "@ff-labs/fff-mcp"]
    }
  }
}
```

### Cursor

Add to your Cursor configuration file:

**macOS/Linux**: `~/.cursor/mcp.json`
**Windows**: `%APPDATA%\Cursor\mcp.json`

```json
{
  "mcpServers": {
    "fff": {
      "command": "npx",
      "args": ["-y", "@ff-labs/fff-mcp"]
    }
  }
}
```

### Custom Database Paths

By default, fff-mcp shares frecency data with fff.nvim if installed, falling back to `~/.fff/`. To use custom database paths:

```json
{
  "mcpServers": {
    "fff": {
      "command": "npx",
      "args": [
        "-y",
        "@ff-labs/fff-mcp",
        "--frecency-db",
        "/path/to/frecency.mdb",
        "--history-db",
        "/path/to/history.mdb"
      ]
    }
  }
}
```

## Available Tools

### `find_files`
Fast file name search with fuzzy matching and frecency ranking.

**Use when**: You know the filename or path you're looking for.

**Example queries**:
- `README.md` - exact filename
- `indx.ts` - fuzzy match for "index.ts"
- `src/` - files in directory
- `*.rs` - extension filtering
- `crs/fff-core/lib` - fuzzy path matching

### `grep`
Literal text search across files with frecency-ordered results and automatic fuzzy fallback.

**Use when**: Searching for exact text or identifiers in code.

**Example queries**:
- `frecency_boost` - find exact identifier
- `*.ts FileFinder` - constrained to TypeScript files
- `src/ function` - search within directory
- `!test/ pattern` - exclude test directories

**Features**: Smart case (case-insensitive for lowercase queries), binary file filtering, context lines, fuzzy fallback for typos.

### `regex_grep`
Regular expression search with graceful fallback to literal matching on invalid regex.

**Use when**: You need pattern matching with wildcards, word boundaries, or alternation.

**Example queries**:
- `function\s+\w+` - functions with any name
- `class\s+(Foo|Bar)` - specific class names
- `\bapi\b` - word boundary matching

**Features**: Full regex support, automatic fallback with helpful error messages.

### `multi_grep`
Efficient multi-pattern search using Aho-Corasick algorithm for OR logic.

**Use when**: Searching for multiple terms where any match is relevant.

**Example patterns**: `["useState", "useEffect", "useContext"]` - find any React hook usage.

**Features**: Single-pass multi-pattern matching, automatic fallback to individual grep, separate constraints parameter.

## Benchmark Results

Tested on a 48k+ file repository (fff.nvim codebase):

| Query Type           | fff-mcp P50 | Native P50 | Speedup | fff Size | Native Size | Token ↓ | Accuracy |
|----------------------|-------------|------------|---------|----------|-------------|---------|----------|
| Exact filename       |       2.1ms |      8.3ms |   3.95x |     124b |        891b |     86% |        ✓ |
| Fuzzy filename       |       3.4ms |     12.7ms |   3.74x |     142b |       1203b |     88% |        ✓ |
| Directory prefix     |       1.8ms |      6.2ms |   3.44x |     456b |       3821b |     88% |        ✓ |
| Extension filter     |       4.2ms |     15.1ms |   3.60x |     892b |       8234b |     89% |        ✓ |
| Fuzzy path           |       5.1ms |     18.4ms |   3.61x |     167b |       1456b |     89% |        ✓ |
| Common word grep     |      12.3ms |     45.2ms |   3.67x |    1234b |      12456b |     90% |        ✓ |
| Rare identifier      |       8.7ms |     38.9ms |   4.47x |     312b |       4123b |     92% |        ✓ |
| Multi-word literal   |      11.2ms |     42.1ms |   3.76x |     567b |       5789b |     90% |        ✓ |
| Constrained grep     |       7.4ms |     29.3ms |   3.96x |     423b |       3891b |     89% |        ✓ |
| Fuzzy fallback       |      15.8ms |     51.2ms |   3.24x |     289b |       4567b |     94% |        ✓ |
| Regex pattern        |      13.5ms |     48.7ms |   3.61x |     678b |       6234b |     89% |        ✓ |
| Regex alternation    |      16.2ms |     52.3ms |   3.23x |     891b |       7823b |     89% |        ✓ |
| Multi-pattern OR     |      18.9ms |     67.4ms |   3.57x |    1123b |      11234b |     90% |        ✓ |
| **Average**          |   **9.3ms** | **33.5ms** | **3.6x**| **561b** |    **5594b**| **90%** |      ✓ |

### Key Insights

✓ **3.6x faster** than native tools on average
✓ **90% token reduction** through compact output and relevance ranking
✓ **100% accuracy** - expected files consistently appear in top 5 results
✓ Frecency ranking surfaces relevant files without over-constraining queries
✓ Fuzzy matching recovers from typos automatically
✓ Git-aware scoring prioritizes actively-changed files

## Shared Frecency with fff.nvim

If you use [fff.nvim](https://github.com/dmtrKovalenko/fff.nvim) (Neovim file picker), fff-mcp automatically shares the same frecency database. This means:

- Files you open frequently in Neovim appear higher in AI search results
- Your file access patterns improve AI assistant accuracy over time
- Zero configuration - automatic detection of Neovim data directories

**Database locations** (auto-detected):
- **Frecency**: `~/.cache/nvim/fff_nvim` (macOS/Linux) or `%LOCALAPPDATA%\nvim-data\fff_nvim` (Windows)
- **History**: `~/.local/share/nvim/fff_queries` (macOS/Linux) or `%LOCALAPPDATA%\nvim-data\fff_queries` (Windows)

**Fallback** (if Neovim directories don't exist): `~/.fff/frecency.mdb` and `~/.fff/history.mdb`

## Workspace Support

fff-mcp automatically detects monorepo workspace boundaries:

- **npm/yarn/pnpm**: Reads `package.json` `workspaces` field
- **Cargo**: Detects `[workspace]` in `Cargo.toml`
- **Go modules**: Finds `go.work` or multiple `go.mod` files

When workspaces are detected, use directory constraints to scope searches:

```
grep(pattern: 'packages/api/ AuthService')    → search only api package
find_files(query: 'packages/ui/ *.tsx')       → find tsx files in ui package
```

## Troubleshooting

### "Binary not found" error

**Symptom**: `Error: Cannot find fff-mcp binary`

**Fix**: Ensure the platform-specific package is installed:

```bash
# Check what was installed
npm list -g @ff-labs/fff-mcp

# Force reinstall
npm uninstall -g @ff-labs/fff-mcp
npm install -g @ff-labs/fff-mcp
```

**Supported platforms**: macOS ARM64, macOS x64, Linux x64, Windows x64

### "Database permission denied" error

**Symptom**: `Error: Cannot open database: Permission denied`

**Fix**: Check database directory permissions:

```bash
# macOS/Linux
ls -la ~/.cache/nvim/
chmod 755 ~/.cache/nvim/fff_nvim

# Or use custom paths with write permissions
fff-mcp --frecency-db ~/my-project/.fff/frecency.mdb --history-db ~/my-project/.fff/history.mdb
```

### MCP server not starting in Claude Code/Cursor

**Symptom**: fff tools don't appear in AI assistant

**Fix**:
1. Check MCP configuration syntax (valid JSON)
2. Verify `npx` is in PATH: `which npx` (macOS/Linux) or `where npx` (Windows)
3. Test manually: `npx @ff-labs/fff-mcp` (should show JSON-RPC initialization)
4. Check logs:
   - **Claude Code**: `~/.config/claude/logs/`
   - **Cursor**: `~/.cursor/logs/`

### Search results seem inaccurate

**Symptom**: Expected files don't appear in top results

**Cause**: Frecency database is cold (no usage history yet)

**Fix**: Use the tools more! Frecency improves over time as you access files. To bootstrap:
1. Open files you work with frequently in Neovim (if using fff.nvim)
2. Search for files multiple times - repeated searches boost frecency scores
3. Be patient - accuracy improves after 1-2 days of normal usage

### Performance slower than expected

**Symptom**: Searches take >100ms on medium repos

**Potential causes**:
- Large binary files in repository (videos, databases)
- Network-mounted filesystems
- Antivirus scanning interfering with file access

**Fix**:
- Ensure `.gitignore` excludes large binary directories
- Use local filesystem for best performance
- Add antivirus exceptions for project directories

## License

MIT License - see [LICENSE](../../LICENSE) for details.

## Contributing

Issues and pull requests welcome at [github.com/dmtrKovalenko/fff.nvim](https://github.com/dmtrKovalenko/fff.nvim)

## Related Projects

- **[fff.nvim](https://github.com/dmtrKovalenko/fff.nvim)** - Neovim file picker with the same fuzzy matching and frecency engine
- **[blink.cmp](https://github.com/Saghen/blink.cmp)** - Inspiration for the fuzzy matching algorithm
