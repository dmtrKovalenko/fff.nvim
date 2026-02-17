# fff - Fast File Finder

High-performance fuzzy file finder for Bun, powered by Rust. Perfect for LLM agent tools that need to search through codebases.

## Features

- **Blazing fast** - Rust-powered fuzzy search with parallel processing
- **Smart ranking** - Frecency-based scoring (frequency + recency)
- **Git-aware** - Shows file git status in results
- **Query history** - Learns from your search patterns
- **Type-safe** - Full TypeScript support with Result types

## Installation

```bash
bun add @ff-labs/bun
```

The correct native binary for your platform is installed automatically via platform-specific packages (e.g. `@ff-labs/fff-bun-darwin-arm64`, `@ff-labs/fff-bun-linux-x64-gnu`). No GitHub downloads are needed.

### Supported Platforms

| Platform | Architecture | Package |
|----------|-------------|---------|
| macOS | ARM64 (Apple Silicon) | `@ff-labs/fff-bun-darwin-arm64` |
| macOS | x64 (Intel) | `@ff-labs/fff-bun-darwin-x64` |
| Linux | x64 (glibc) | `@ff-labs/fff-bun-linux-x64-gnu` |
| Linux | ARM64 (glibc) | `@ff-labs/fff-bun-linux-arm64-gnu` |
| Linux | x64 (musl) | `@ff-labs/fff-bun-linux-x64-musl` |
| Linux | ARM64 (musl) | `@ff-labs/fff-bun-linux-arm64-musl` |
| Windows | x64 | `@ff-labs/fff-bun-win32-x64` |
| Windows | ARM64 | `@ff-labs/fff-bun-win32-arm64` |

If the platform package isn't available, the postinstall script will attempt to download from GitHub releases as a fallback.

## Quick Start

```typescript
import { FileFinder } from "fff";

// Initialize with a directory
const result = FileFinder.init({ basePath: "/path/to/project" });
if (!result.ok) {
  console.error(result.error);
  process.exit(1);
}

// Wait for initial scan
FileFinder.waitForScan(5000);

// Search for files
const search = FileFinder.search("main.ts");
if (search.ok) {
  for (const item of search.value.items) {
    console.log(item.relativePath);
  }
}

// Cleanup when done
FileFinder.destroy();
```

## API Reference

### `FileFinder.init(options)`

Initialize the file finder.

```typescript
interface InitOptions {
  basePath: string;           // Directory to index (required)
  frecencyDbPath?: string;    // Frecency DB path (omit to skip frecency)
  historyDbPath?: string;     // History DB path (omit to skip query tracking)
  useUnsafeNoLock?: boolean;  // Faster but less safe DB mode
}

const result = FileFinder.init({ basePath: "/my/project" });
```

### `FileFinder.search(query, options?)`

Search for files.

```typescript
interface SearchOptions {
  maxThreads?: number;          // Parallel threads (0 = auto)
  currentFile?: string;         // Deprioritize this file
  comboBoostMultiplier?: number; // Query history boost
  minComboCount?: number;        // Min history matches
  pageIndex?: number;            // Pagination offset
  pageSize?: number;             // Results per page
}

const result = FileFinder.search("main.ts", { pageSize: 10 });
if (result.ok) {
  console.log(`Found ${result.value.totalMatched} files`);
}
```

### Query Syntax

- `foo bar` - Match files containing "foo" and "bar"
- `src/` - Match files in src directory
- `file.ts:42` - Match file.ts with line 42
- `file.ts:42:10` - Match with line and column

### `FileFinder.trackAccess(filePath)`

Track file access for frecency scoring.

```typescript
// Call when user opens a file
FileFinder.trackAccess("/path/to/file.ts");
```

### `FileFinder.liveGrep(query, options?)`

Search file contents with SIMD-accelerated matching.

```typescript
interface GrepOptions {
  maxFileSize?: number;        // Max file size in bytes (default: 10MB)
  maxMatchesPerFile?: number;  // Max matches per file (default: 200)
  smartCase?: boolean;         // Case-insensitive if all lowercase (default: true)
  fileOffset?: number;         // Pagination offset (default: 0)
  pageLimit?: number;          // Max matches to return (default: 50)
  mode?: "plain" | "regex" | "fuzzy"; // Search mode (default: "plain")
  timeBudgetMs?: number;       // Time limit in ms, 0 = unlimited (default: 0)
}

// Plain text search
const result = FileFinder.liveGrep("TODO", { pageLimit: 20 });
if (result.ok) {
  for (const match of result.value.items) {
    console.log(`${match.relativePath}:${match.lineNumber}: ${match.lineContent}`);
  }
}

// Regex search
const regexResult = FileFinder.liveGrep("fn\\s+\\w+", { mode: "regex" });

// Fuzzy search
const fuzzyResult = FileFinder.liveGrep("imprt recat", { mode: "fuzzy" });

// Pagination
const page1 = FileFinder.liveGrep("error");
if (page1.ok && page1.value.nextCursor) {
  const page2 = FileFinder.liveGrep("error", {
    cursor: page1.value.nextCursor,
  });
}

// With file constraints
const tsOnly = FileFinder.liveGrep("*.ts useState");
const srcOnly = FileFinder.liveGrep("src/ handleClick");
```

### `FileFinder.trackQuery(query, selectedFile)`

Track query completion for smart suggestions.

```typescript
// Call when user selects a file from search
FileFinder.trackQuery("main", "/path/to/main.ts");
```

### `FileFinder.healthCheck(testPath?)`

Get diagnostic information.

```typescript
const health = FileFinder.healthCheck();
if (health.ok) {
  console.log(`Version: ${health.value.version}`);
  console.log(`Indexed: ${health.value.filePicker.indexedFiles} files`);
}
```

### Other Methods

- `FileFinder.liveGrep(query, options?)` - Search file contents
- `FileFinder.scanFiles()` - Trigger rescan
- `FileFinder.isScanning()` - Check scan status
- `FileFinder.getScanProgress()` - Get scan progress
- `FileFinder.waitForScan(timeoutMs)` - Wait for scan
- `FileFinder.reindex(newPath)` - Change indexed directory
- `FileFinder.refreshGitStatus()` - Refresh git cache
- `FileFinder.getHistoricalQuery(offset)` - Get past queries
- `FileFinder.destroy()` - Cleanup resources

## Result Types

All methods return a `Result<T>` type for explicit error handling:

```typescript
type Result<T> = 
  | { ok: true; value: T }
  | { ok: false; error: string };

const result = FileFinder.search("foo");
if (result.ok) {
  // result.value is SearchResult
} else {
  // result.error is string
}
```

## Search Result Types

```typescript
interface SearchResult {
  items: FileItem[];
  scores: Score[];
  totalMatched: number;
  totalFiles: number;
  location?: Location;
}

interface FileItem {
  path: string;
  relativePath: string;
  fileName: string;
  size: number;
  modified: number;
  gitStatus: string; // 'clean', 'modified', 'untracked', etc.
}
```

## Grep Result Types

```typescript
interface GrepResult {
  items: GrepMatch[];
  totalMatched: number;
  totalFilesSearched: number;
  totalFiles: number;
  filteredFileCount: number;
  nextCursor: GrepCursor | null; // Pass to options.cursor for next page
  regexFallbackError?: string;   // Set if regex was invalid
}

interface GrepMatch {
  path: string;
  relativePath: string;
  fileName: string;
  gitStatus: string;
  lineNumber: number;    // 1-based
  col: number;           // 0-based byte column
  byteOffset: number;    // Absolute byte offset in file
  lineContent: string;   // The matched line text
  matchRanges: [number, number][]; // Byte offsets for highlighting
  fuzzyScore?: number;   // Only in fuzzy mode
}
```

## Building from Source

If prebuilt binaries aren't available for your platform:

```bash
# Clone the repository
git clone https://github.com/dmtrKovalenko/fff.nvim
cd fff.nvim

# Build the C library
cargo build --release -p fff-c

# The binary will be at target/release/libfff_c.{so,dylib,dll}
```

## CLI Tools

```bash
# Download binary manually (fallback if npm package unavailable)
bunx fff download [tag]

# Show platform info and binary location
bunx fff info
```

## License

MIT
