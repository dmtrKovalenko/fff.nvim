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
 * const search = finder.fileSearch("main.ts");
 * if (search.ok) {
 *   for (const item of search.value.items) {
 *     console.log(item.relativePath);
 *   }
 * }
 *
 * // Cleanup when done
 * finder.destroy();
 * ```
 *
 * @packageDocumentation
 */

export {
  binaryExists,
  findBinary,
} from "./download";
export { FileFinder } from "./finder";

export {
  getLibExtension,
  getLibFilename,
  getNpmPackageName,
  getTriple,
} from "./platform";

export type {
  DbHealth,
  DirItem,
  DirSearchOptions,
  DirSearchResult,
  FileItem,
  GrepCursor,
  GrepMatch,
  GrepMode,
  GrepOptions,
  GrepResult,
  HealthCheck,
  InitOptions,
  Location,
  MixedItem,
  MixedSearchResult,
  MultiGrepOptions,
  Result,
  ScanProgress,
  Score,
  SearchOptions,
  SearchResult,
} from "./types";
// Result helpers
export { err, ok } from "./types";
