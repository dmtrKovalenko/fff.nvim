/**
 * fff - Fast File Finder
 *
 * High-performance fuzzy file finder for Bun, powered by Rust.
 * Perfect for LLM agent tools that need to search through codebases.
 *
 * Each `FileFinder` instance is backed by an independent native file picker.
 * Create as many as you need and destroy them when done.
 *
 * @example
 * ```typescript
 * import { FileFinder } from "fff";
 *
 * // Create a file finder instance
 * const result = FileFinder.create({ basePath: "/path/to/project" });
 * if (!result.ok) {
 *   console.error(result.error);
 *   process.exit(1);
 * }
 * const finder = result.value;
 *
 * // Wait for initial scan
 * finder.waitForScan(5000);
 *
 * // Search for files
 * const search = finder.search("main.ts");
 * if (search.ok) {
 *   for (const item of search.value.items) {
 *     console.log(item.relativePath);
 *   }
 * }
 *
 * // Track file access (for frecency)
 * finder.trackAccess("/path/to/project/src/main.ts");
 *
 * // Cleanup when done
 * finder.destroy();
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
export {
  getTriple,
  getLibExtension,
  getLibFilename,
  getNpmPackageName,
} from "./platform";
