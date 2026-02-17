/**
 * Result type for all operations - follows the Result pattern
 */
export type Result<T> =
  | { ok: true; value: T }
  | { ok: false; error: string };

/**
 * Helper to create a successful result
 */
export function ok<T>(value: T): Result<T> {
  return { ok: true, value };
}

/**
 * Helper to create an error result
 */
export function err<T>(error: string): Result<T> {
  return { ok: false, error };
}

/**
 * Initialization options for the file finder
 */
export interface InitOptions {
  /** Base directory to index (required) */
  basePath: string;
  /** Path to frecency database (optional, omit to skip frecency initialization) */
  frecencyDbPath?: string;
  /** Path to query history database (optional, omit to skip query tracker initialization) */
  historyDbPath?: string;
  /** Use unsafe no-lock mode for databases (optional, defaults to false) */
  useUnsafeNoLock?: boolean;
  /**
   * Pre-populate mmap caches for all files after the initial scan completes.
   * When enabled, the first grep search will be as fast as subsequent ones
   * at the cost of a longer scan time and higher initial memory usage.
   * (default: false)
   */
  warmupMmapCache?: boolean;
}

/**
 * Search options for fuzzy file search
 */
export interface SearchOptions {
  /** Maximum threads for parallel search (0 = auto) */
  maxThreads?: number;
  /** Current file path (for deprioritization in results) */
  currentFile?: string;
  /** Combo boost score multiplier (default: 100) */
  comboBoostMultiplier?: number;
  /** Minimum combo count for boost (default: 3) */
  minComboCount?: number;
  /** Page index for pagination (default: 0) */
  pageIndex?: number;
  /** Page size for pagination (default: 100) */
  pageSize?: number;
}

/**
 * A file item in search results
 */
export interface FileItem {
  /** Absolute path to the file */
  path: string;
  /** Path relative to the indexed directory */
  relativePath: string;
  /** File name only */
  fileName: string;
  /** File size in bytes */
  size: number;
  /** Last modified timestamp (Unix seconds) */
  modified: number;
  /** Frecency score based on access patterns */
  accessFrecencyScore: number;
  /** Frecency score based on modification time */
  modificationFrecencyScore: number;
  /** Combined frecency score */
  totalFrecencyScore: number;
  /** Git status: 'clean', 'modified', 'untracked', 'staged_new', etc. */
  gitStatus: string;
}

/**
 * Score breakdown for a search result
 */
export interface Score {
  /** Total combined score */
  total: number;
  /** Base fuzzy match score */
  baseScore: number;
  /** Bonus for filename match */
  filenameBonus: number;
  /** Bonus for special filenames (index.ts, main.rs, etc.) */
  specialFilenameBonus: number;
  /** Boost from frecency */
  frecencyBoost: number;
  /** Penalty for distance in path */
  distancePenalty: number;
  /** Penalty if this is the current file */
  currentFilePenalty: number;
  /** Boost from query history combo matching */
  comboMatchBoost: number;
  /** Whether this was an exact match */
  exactMatch: boolean;
  /** Type of match: 'fuzzy', 'exact', 'prefix', etc. */
  matchType: string;
}

/**
 * Location in file (from query like "file.ts:42")
 */
export type Location =
  | { type: "line"; line: number }
  | { type: "position"; line: number; col: number }
  | {
      type: "range";
      start: { line: number; col: number };
      end: { line: number; col: number };
    };

/**
 * Search result from fuzzy file search
 */
export interface SearchResult {
  /** Matched file items */
  items: FileItem[];
  /** Corresponding scores for each item */
  scores: Score[];
  /** Total number of files that matched */
  totalMatched: number;
  /** Total number of indexed files */
  totalFiles: number;
  /** Location parsed from query (e.g., "file.ts:42:10") */
  location?: Location;
}

/**
 * Scan progress information
 */
export interface ScanProgress {
  /** Number of files scanned so far */
  scannedFilesCount: number;
  /** Whether a scan is currently in progress */
  isScanning: boolean;
}

/**
 * Database health information
 */
export interface DbHealth {
  /** Path to the database */
  path: string;
  /** Size of the database on disk in bytes */
  diskSize: number;
}

/**
 * Health check result
 */
export interface HealthCheck {
  /** Library version */
  version: string;
  /** Git integration status */
  git: {
    /** Whether git2 library is available */
    available: boolean;
    /** Whether a git repository was found */
    repositoryFound: boolean;
    /** Git working directory path */
    workdir?: string;
    /** libgit2 version string */
    libgit2Version: string;
    /** Error message if git detection failed */
    error?: string;
  };
  /** File picker status */
  filePicker: {
    /** Whether the file picker is initialized */
    initialized: boolean;
    /** Base path being indexed */
    basePath?: string;
    /** Whether a scan is in progress */
    isScanning?: boolean;
    /** Number of indexed files */
    indexedFiles?: number;
    /** Error message if there's an issue */
    error?: string;
  };
  /** Frecency database status */
  frecency: {
    /** Whether frecency tracking is initialized */
    initialized: boolean;
    /** Database health information */
    dbHealthcheck?: DbHealth;
    /** Error message if there's an issue */
    error?: string;
  };
  /** Query tracker status */
  queryTracker: {
    /** Whether query tracking is initialized */
    initialized: boolean;
    /** Database health information */
    dbHealthcheck?: DbHealth;
    /** Error message if there's an issue */
    error?: string;
  };
}

/**
 * Internal: Options format sent to Rust FFI
 * @internal
 */
export interface InitOptionsInternal {
  base_path: string;
  frecency_db_path?: string;
  history_db_path?: string;
  use_unsafe_no_lock: boolean;
  warmup_mmap_cache: boolean;
}

/**
 * Internal: Search options format sent to Rust FFI
 * @internal
 */
export interface SearchOptionsInternal {
  max_threads?: number;
  current_file?: string;
  combo_boost_multiplier?: number;
  min_combo_count?: number;
  page_index?: number;
  page_size?: number;
}

/**
 * Convert public InitOptions to internal format
 * @internal
 */
export function toInternalInitOptions(opts: InitOptions): InitOptionsInternal {
  return {
    base_path: opts.basePath,
    frecency_db_path: opts.frecencyDbPath,
    history_db_path: opts.historyDbPath,
    use_unsafe_no_lock: opts.useUnsafeNoLock ?? false,
    warmup_mmap_cache: opts.warmupMmapCache ?? false,
  };
}

/**
 * Convert public SearchOptions to internal format
 * @internal
 */
export function toInternalSearchOptions(
  opts?: SearchOptions
): SearchOptionsInternal {
  return {
    max_threads: opts?.maxThreads,
    current_file: opts?.currentFile,
    combo_boost_multiplier: opts?.comboBoostMultiplier,
    min_combo_count: opts?.minComboCount,
    page_index: opts?.pageIndex,
    page_size: opts?.pageSize,
  };
}

// ============================================================================
// Grep (live content search) types
// ============================================================================

/**
 * Grep search mode
 */
export type GrepMode = "plain" | "regex" | "fuzzy";

/**
 * Opaque pagination cursor for grep results.
 * Pass this to `GrepOptions.cursor` to fetch the next page.
 * Do not construct or modify this — use the `nextCursor` from a previous `GrepResult`.
 */
export interface GrepCursor {
  /** @internal */
  readonly __brand: "GrepCursor";
  /** @internal */
  readonly _offset: number;
}

/**
 * @internal Create a GrepCursor from a raw file offset.
 */
export function createGrepCursor(offset: number): GrepCursor {
  return { __brand: "GrepCursor" as const, _offset: offset };
}

/**
 * Options for live grep (content search)
 *
 * Files are searched sequentially in frecency order (most recently/frequently
 * accessed first). The engine collects matching lines across files until
 * `pageLimit` total matches are reached, then stops and returns a
 * `nextCursor` for fetching the next page.
 */
export interface GrepOptions {
  /** Maximum file size to search in bytes. Files larger than this are skipped. (default: 10MB) */
  maxFileSize?: number;
  /** Maximum matching lines to collect from a single file (default: 200) */
  maxMatchesPerFile?: number;
  /** Smart case: case-insensitive when the query is all lowercase, case-sensitive otherwise (default: true) */
  smartCase?: boolean;
  /**
   * Pagination cursor from a previous `GrepResult.nextCursor`.
   * Omit (or pass `null`) for the first page.
   */
  cursor?: GrepCursor | null;
  /**
   * Maximum total number of matching lines to return across all files.
   * The engine walks files in frecency order, accumulating matches until this
   * limit is reached, then truncates and stops.
   *
   * Pagination is file-based, not match-based: if a single file produces more
   * matches than the remaining capacity, the excess matches from that file are
   * dropped and the next page resumes from the *next* file. This means some
   * matches at the boundary may be skipped, but it guarantees no duplicates
   * across pages and requires no server-side cursor state.
   *
   * Use `nextCursor` from the result to fetch the next page. (default: 50)
   */
  pageLimit?: number;
  /** Search mode (default: "plain") */
  mode?: GrepMode;
  /**
   * Maximum wall-clock time in milliseconds to spend searching before returning
   * partial results. The engine will still return at least `pageLimit / 2` matches
   * (if available) before honoring the budget. 0 = unlimited. (default: 0)
   */
  timeBudgetMs?: number;
}

/**
 * A single grep match with file and line information
 */
export interface GrepMatch {
  /** Absolute path to the file */
  path: string;
  /** Path relative to the indexed directory */
  relativePath: string;
  /** File name only */
  fileName: string;
  /** Git status */
  gitStatus: string;
  /** File size in bytes */
  size: number;
  /** Last modified timestamp (Unix seconds) */
  modified: number;
  /** Whether the file is binary */
  isBinary: boolean;
  /** Combined frecency score */
  totalFrecencyScore: number;
  /** Access-based frecency score */
  accessFrecencyScore: number;
  /** Modification-based frecency score */
  modificationFrecencyScore: number;
  /** 1-based line number of the match */
  lineNumber: number;
  /** 0-based byte column of first match start */
  col: number;
  /** Absolute byte offset of the matched line from file start */
  byteOffset: number;
  /** The matched line text (may be truncated) */
  lineContent: string;
  /** Byte offset pairs [start, end] within lineContent for highlighting */
  matchRanges: [number, number][];
  /** Fuzzy match score (only in fuzzy mode) */
  fuzzyScore?: number;
}

/**
 * Result from a grep search
 */
export interface GrepResult {
  /** Matched items with file and line information. At most `pageLimit` entries. */
  items: GrepMatch[];
  /** Total number of matches collected (equal to items.length unless truncated by pageLimit) */
  totalMatched: number;
  /** Number of files actually opened and searched in this call */
  totalFilesSearched: number;
  /** Total number of indexed files (before any filtering) */
  totalFiles: number;
  /** Number of files eligible for search after filtering out binary files, oversized files, and constraint mismatches */
  filteredFileCount: number;
  /**
   * Cursor for the next page, or `null` if all eligible files have been searched.
   * Pass this as `GrepOptions.cursor` to continue from where this call left off.
   */
  nextCursor: GrepCursor | null;
  /** When regex mode fails to compile the pattern, the engine falls back to literal matching and this field contains the compilation error */
  regexFallbackError?: string;
}

/**
 * Internal: Grep options format sent to Rust FFI
 * @internal
 */
export interface GrepOptionsInternal {
  max_file_size?: number;
  max_matches_per_file?: number;
  smart_case?: boolean;
  file_offset?: number;
  page_limit?: number;
  mode?: string;
  time_budget_ms?: number;
}

/**
 * Convert public GrepOptions to internal format
 * @internal
 */
export function toInternalGrepOptions(
  opts?: GrepOptions
): GrepOptionsInternal {
  return {
    max_file_size: opts?.maxFileSize,
    max_matches_per_file: opts?.maxMatchesPerFile,
    smart_case: opts?.smartCase,
    file_offset: opts?.cursor?._offset ?? 0,
    page_limit: opts?.pageLimit,
    mode: opts?.mode,
    time_budget_ms: opts?.timeBudgetMs,
  };
}
