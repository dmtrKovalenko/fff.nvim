/**
 * FileFinder - High-level API for the fff file finder
 *
 * This class provides a type-safe, ergonomic API for file finding operations.
 * All methods return Result types for explicit error handling.
 */

import {
  ffiInit,
  ffiDestroy,
  ffiSearch,
  ffiScanFiles,
  ffiIsScanning,
  ffiGetScanProgress,
  ffiWaitForScan,
  ffiRestartIndex,
  ffiTrackAccess,
  ffiRefreshGitStatus,
  ffiTrackQuery,
  ffiGetHistoricalQuery,
  ffiHealthCheck,
  ffiShortenPath,
  ensureLoaded,
  isAvailable,
} from "./ffi";

import type {
  Result,
  InitOptions,
  SearchOptions,
  SearchResult,
  ScanProgress,
  HealthCheck,
} from "./types";

import { err, toInternalInitOptions, toInternalSearchOptions } from "./types";

/**
 * FileFinder - Fast file finder with fuzzy search
 *
 * @example
 * ```typescript
 * import { FileFinder } from "fff";
 *
 * // Initialize
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
 * // Cleanup
 * FileFinder.destroy();
 * ```
 */
export class FileFinder {
  private static initialized = false;

  /**
   * Initialize the file finder with the given options.
   *
   * @param options - Initialization options
   * @returns Result indicating success or failure
   *
   * @example
   * ```typescript
   * // Basic initialization
   * FileFinder.init({ basePath: "/path/to/project" });
   *
   * // With custom database paths
   * FileFinder.init({
   *   basePath: "/path/to/project",
   *   frecencyDbPath: "/custom/frecency.mdb",
   *   historyDbPath: "/custom/history.mdb",
   * });
   *
   * // Minimal mode (no databases - just omit db paths)
   * FileFinder.init({ basePath: "/path/to/project" });
   * ```
   */
  static init(options: InitOptions): Result<void> {
    const internalOpts = toInternalInitOptions(options);
    const result = ffiInit(JSON.stringify(internalOpts));

    if (result.ok) {
      this.initialized = true;
    }

    return result;
  }

  /**
   * Destroy and clean up all resources.
   *
   * Call this when you're done using the file finder to free memory
   * and stop background file watching.
   */
  static destroy(): Result<void> {
    const result = ffiDestroy();
    if (result.ok) {
      this.initialized = false;
    }
    return result;
  }

  /**
   * Search for files matching the query.
   *
   * The query supports fuzzy matching and special syntax:
   * - `foo bar` - Match files containing "foo" and "bar"
   * - `src/` - Match files in src directory
   * - `file.ts:42` - Match file.ts with line 42
   * - `file.ts:42:10` - Match file.ts with line 42, column 10
   *
   * @param query - Search query string
   * @param options - Search options
   * @returns Search results with matched files and scores
   *
   * @example
   * ```typescript
   * const result = FileFinder.search("main.ts", { pageSize: 10 });
   * if (result.ok) {
   *   console.log(`Found ${result.value.totalMatched} files`);
   *   for (const item of result.value.items) {
   *     console.log(item.relativePath);
   *   }
   * }
   * ```
   */
  static search(query: string, options?: SearchOptions): Result<SearchResult> {
    if (!this.initialized) {
      return err("FileFinder not initialized. Call FileFinder.init() first.");
    }

    const internalOpts = toInternalSearchOptions(options);
    const result = ffiSearch(query, JSON.stringify(internalOpts));

    if (!result.ok) {
      return result;
    }

    // The FFI returns the search result already parsed
    return result as Result<SearchResult>;
  }

  /**
   * Trigger a rescan of the indexed directory.
   *
   * This is useful after major file system changes that the
   * background watcher might have missed.
   */
  static scanFiles(): Result<void> {
    if (!this.initialized) {
      return err("FileFinder not initialized. Call FileFinder.init() first.");
    }
    return ffiScanFiles();
  }

  /**
   * Check if a scan is currently in progress.
   */
  static isScanning(): boolean {
    if (!this.initialized) return false;
    return ffiIsScanning();
  }

  /**
   * Get the current scan progress.
   */
  static getScanProgress(): Result<ScanProgress> {
    if (!this.initialized) {
      return err("FileFinder not initialized. Call FileFinder.init() first.");
    }
    return ffiGetScanProgress() as Result<ScanProgress>;
  }

  /**
   * Wait for the initial file scan to complete.
   *
   * @param timeoutMs - Maximum time to wait in milliseconds (default: 5000)
   * @returns true if scan completed, false if timed out
   *
   * @example
   * ```typescript
   * FileFinder.init({ basePath: "/path/to/project" });
   * const completed = FileFinder.waitForScan(10000);
   * if (!completed.ok || !completed.value) {
   *   console.warn("Scan did not complete in time");
   * }
   * ```
   */
  static waitForScan(timeoutMs: number = 5000): Result<boolean> {
    if (!this.initialized) {
      return err("FileFinder not initialized. Call FileFinder.init() first.");
    }
    return ffiWaitForScan(timeoutMs);
  }

  /**
   * Change the indexed directory to a new path.
   *
   * This stops the current file watcher and starts indexing the new directory.
   *
   * @param newPath - New directory path to index
   */
  static reindex(newPath: string): Result<void> {
    if (!this.initialized) {
      return err("FileFinder not initialized. Call FileFinder.init() first.");
    }
    return ffiRestartIndex(newPath);
  }

  /**
   * Track file access for frecency scoring.
   *
   * Call this when a user opens a file to improve future search rankings.
   *
   * @param filePath - Absolute path to the accessed file
   */
  static trackAccess(filePath: string): Result<boolean> {
    if (!this.initialized) {
      return { ok: true, value: false };
    }
    return ffiTrackAccess(filePath);
  }

  /**
   * Refresh the git status cache.
   *
   * @returns Number of files with updated git status
   */
  static refreshGitStatus(): Result<number> {
    if (!this.initialized) {
      return err("FileFinder not initialized. Call FileFinder.init() first.");
    }
    return ffiRefreshGitStatus();
  }

  /**
   * Track query completion for smart suggestions.
   *
   * Call this when a user selects a file from search results.
   * This helps improve future search rankings for similar queries.
   *
   * @param query - The search query that was used
   * @param selectedFilePath - The file path that was selected
   */
  static trackQuery(query: string, selectedFilePath: string): Result<boolean> {
    if (!this.initialized) {
      return { ok: true, value: false };
    }
    return ffiTrackQuery(query, selectedFilePath);
  }

  /**
   * Get a historical query by offset.
   *
   * @param offset - Offset from most recent (0 = most recent)
   * @returns The historical query string, or null if not found
   */
  static getHistoricalQuery(offset: number): Result<string | null> {
    if (!this.initialized) {
      return { ok: true, value: null };
    }
    return ffiGetHistoricalQuery(offset);
  }

  /**
   * Get health check information.
   *
   * Useful for debugging and verifying the file finder is working correctly.
   *
   * @param testPath - Optional path to test git repository detection
   */
  static healthCheck(testPath?: string): Result<HealthCheck> {
    return ffiHealthCheck(testPath || "") as Result<HealthCheck>;
  }

  /**
   * Shorten a file path for display.
   *
   * @param path - Path to shorten
   * @param maxSize - Maximum length
   * @param strategy - Shortening strategy: 'middle_number', 'beginning', or 'end'
   */
  static shortenPath(
    path: string,
    maxSize: number,
    strategy: "middle_number" | "beginning" | "end" = "middle_number",
  ): Result<string> {
    return ffiShortenPath(path, maxSize, strategy);
  }

  /**
   * Check if the native library is available.
   */
  static isAvailable(): boolean {
    return isAvailable();
  }

  /**
   * Ensure the native library is loaded.
   *
   * This will download the binary if needed and load it.
   * Useful for preloading before first use.
   */
  static async ensureLoaded(): Promise<void> {
    return ensureLoaded();
  }

  /**
   * Check if the file finder is initialized.
   */
  static isInitialized(): boolean {
    return this.initialized;
  }
}
