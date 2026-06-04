# fff - Fast File Finder

High-performance fuzzy file finder for Node.js, powered by Rust. Extremely fast live file, content, and directory search with a typo-resistant algorithm. As well as regex, plain-text, multi-occurrence and typo-resistant content search.

Comes with built-in git status support, frecency access tracking, and a real-time file watcher, content indexing and many more! Designed for LLM agent tools that search through codebases or agentic RAG document search.

Faster than ripgrep & fzf on any workflow that runs more than once per process.

## Installation

```bash
npm install @ff-labs/fff-node
```

The correct native binary for your platform is installed automatically via platform-specific packages (e.g. `@ff-labs/fff-bin-darwin-arm64`, `@ff-labs/fff-bin-linux-x64-gnu`)

### Supported Platforms

| Platform | Architecture          | Package                             |
| -------- | --------------------- | ----------------------------------- |
| macOS    | ARM64 (Apple Silicon) | `@ff-labs/fff-bin-darwin-arm64`     |
| macOS    | x64 (Intel)           | `@ff-labs/fff-bin-darwin-x64`       |
| Linux    | x64 (glibc)           | `@ff-labs/fff-bin-linux-x64-gnu`    |
| Linux    | ARM64 (glibc)         | `@ff-labs/fff-bin-linux-arm64-gnu`  |
| Linux    | x64 (musl)            | `@ff-labs/fff-bin-linux-x64-musl`   |
| Linux    | ARM64 (musl)          | `@ff-labs/fff-bin-linux-arm64-musl` |
| Windows  | x64                   | `@ff-labs/fff-bin-win32-x64`        |
| Windows  | ARM64                 | `@ff-labs/fff-bin-win32-arm64`      |

If the platform package isn't available, the postinstall script will attempt to download from GitHub releases as a fallback.

## Quick Start

Each `FileFinder` instance owns an independent native index. Create one, wait
for the initial scan, then run as many searches as you like.

```typescript
import { FileFinder } from "@ff-labs/fff-node";

// Create an instance bound to a directory
const created = FileFinder.create({ basePath: "/path/to/project" });
if (!created.ok) throw new Error(created.error);

const finder = created.value;

// Wait for the initial scan (async, non-blocking)
await finder.waitForScan(5000);

// 1. Fuzzy file search (typo resistant)
const files = finder.fileSearch("typescropt.ts", { pageSize: 10 });
if (files.ok) {
  for (const item of files.value.items) {
    console.log(item.relativePath, item.gitStatus);
  }
}

// 2. Glob filter — no fuzzy matching, 100% compatible with npm `glob`
const globbed = finder.glob("src/**/*.ts");
if (globbed.ok) console.log(`${globbed.value.totalMatched} TypeScript files`);

// 3. Content search (live grep) with pagination
const grep = finder.grep("TODO", { mode: "plain", pageSize: 20 });
if (grep.ok) {
  for (const m of grep.value.items) {
    console.log(`${m.relativePath}:${m.lineNumber}: ${m.lineContent}`);
  }
}

// 4. Directory search based on the query (typo resistant)
const dirs = finder.directorySearch("components");
if (dirs.ok) console.log(dirs.value.items.map((d) => d.relativePath));

// Free the resources when you don't need a file picker anymore
finder.destroy();
```

## API Reference

Verify the latest API in the local interface at [`./src/fff-api.ts`](./src/fff-api.ts). Every field and type is documented.

### Result Types

All methods return a `Result<T>` type for explicit error handling:

```typescript
type Result<T> = { ok: true; value: T } | { ok: false; error: string };

const result = finder.fileSearch("foo");

if (result.ok) {
  // result.value is SearchResult
} else {
  // result.error is string error message
}
```

This SDK calls a native compiled library for your platform at runtime. This is generally safe — fff is battle-tested and stable, and written in a memory-safe language — but there is a class of errors that can't be caught at the Node.js level. If you hit one, please report an issue!

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

## CLI examples

```bash
# Download binary manually (fallback if npm package unavailable)
npx @ff-labs/fff-node download [tag]

# Show platform info and binary location
npx @ff-labs/fff-node info
```

## License

MIT
