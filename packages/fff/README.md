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
bun add fff
```

The native binary will be downloaded automatically during installation.

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
  frecencyDbPath?: string;    // Custom frecency DB path
  historyDbPath?: string;     // Custom history DB path
  useUnsafeNoLock?: boolean;  // Faster but less safe DB mode
  skipDatabases?: boolean;    // Skip frecency/history (simpler mode)
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

- `FileFinder.scanFiles()` - Trigger rescan
- `FileFinder.isScanning()` - Check scan status
- `FileFinder.getScanProgress()` - Get scan progress
- `FileFinder.waitForScan(timeoutMs)` - Wait for scan
- `FileFinder.restartIndex(newPath)` - Change indexed directory
- `FileFinder.refreshGitStatus()` - Refresh git cache
- `FileFinder.getHistoricalQuery(offset)` - Get past queries
- `FileFinder.shortenPath(path, maxSize, strategy)` - Shorten paths
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
# Download binary manually
bunx fff download [version]

# Show platform info
bunx fff info
```

## License

MIT
