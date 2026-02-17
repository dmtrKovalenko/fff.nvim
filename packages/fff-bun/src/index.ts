/**
 * fff - Fast File Finder
 *
 * High-performance fuzzy file finder for Bun, powered by Rust.
 * Perfect for LLM agent tools that need to search through codebases.
 *
 * @example
 * ```typescript
 * import { FileFinder } from "fff";
 *
 * // Initialize with a directory
 * const result = FileFinder.init({ basePath: "/path/to/project" });
 * if (!result.ok) {
 *   console.error(result.error);
 *   process.exit(1);
 * }
 *
 * // Wait for initial scan
 * FileFinder.waitForScan(5000);
 *
 * // Search for files
 * const search = FileFinder.search("main.ts");
 * if (search.ok) {
 *   for (const item of search.value.items) {
 *     console.log(item.relativePath);
 *   }
 * }
 *
 * // Track file access (for frecency)
 * FileFinder.trackAccess("/path/to/project/src/main.ts");
 *
 * // Cleanup when done
 * FileFinder.destroy();
 * ```
 *
 * @packageDocumentation
 */

// Main API
export { FileFinder } from "./finder";

// Types
export type {
  Result,
  InitOptions,
  SearchOptions,
  FileItem,
  Score,
  Location,
  SearchResult,
  ScanProgress,
  HealthCheck,
  DbHealth,
  GrepMode,
  GrepOptions,
  GrepMatch,
  GrepResult,
  GrepCursor,
} from "./types";

// Result helpers
export { ok, err } from "./types";

// Binary management (for CLI tools)
export {
  downloadBinary,
  ensureBinary,
  binaryExists,
  getBinaryPath,
  findBinary,
} from "./download";

// Platform utilities
export { getTriple, getLibExtension, getLibFilename, getNpmPackageName } from "./platform";
