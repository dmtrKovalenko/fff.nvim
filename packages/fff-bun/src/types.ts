/**
 * Result type for all operations - follows the Result pattern
 */
export type Result<T> = { ok: true; value: T } | { ok: false; error: string };

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
   * Disable mmap cache warmup after the initial scan. When mmap cache is
   * enabled (the default), the first grep search is as fast as subsequent
   * ones at the cost of background resources spent on awarming up the cache
   */
  disableMmapCache?: boolean;
  /**
   * Disable the content index built after the initial scan.
   * Content indexing enables faster content-aware filtering during grep.
   * When omitted, follows `disableMmapCache` for backward compatibility.
   * (default: follows `disableMmapCache`)
   */
  disableContentIndexing?: boolean;
  /**
   * Disable the background file-system watcher. When the watcher is
   * disabled, files are scanned once but not monitored for changes.
   * (default: false)
   */
  disableWatch?: boolean;
  /** enables optimizations for AI agent assistants. Provide as true if running via mcp/agent */
  aiMode?: boolean;
  /**
   * Path to the tracing log file. When set, the shared FFF tracing subscriber
   * is installed on first init and file output is written here. Omit to leave
   * logging uninitialized.
   */
  logFilePath?: string;
  /**
   * Log level for the tracing subscriber: "trace", "debug", "info", "warn",
   * or "error". Defaults to "info". Ignored when `logFilePath` is not set.
   */
  logLevel?: "trace" | "debug" | "info" | "warn" | "error";
  /**
   * Override for the content cache file-count cap. When omitted, the picker
   * auto-sizes the budget from the final scanned file count.
   */
  cacheBudgetMaxFiles?: number;
  /** Override for the content cache byte cap. See `cacheBudgetMaxFiles`. */
  cacheBudgetMaxBytes?: number;
  /** Override for the per-file byte cap in the content cache. */
  cacheBudgetMaxFileSize?: number;
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
 * A directory item in search results
 */
export interface DirItem {
  /** Path relative to the indexed directory (e.g., "src/components/") */
  relativePath: string;
  /** Last path segment (e.g., "components/" for "src/components/") */
  dirName: string;
  /** Maximum access frecency score among direct child files */
  maxAccessFrecency: number;
}

/**
 * Search options for directory search (subset of SearchOptions)
 */
export interface DirSearchOptions {
  /** Maximum threads for parallel search (0 = auto) */
  maxThreads?: number;
  /** Current file path (for distance scoring) */
  currentFile?: string;
  /** Page index for pagination (default: 0) */
  pageIndex?: number;
  /** Page size for pagination (default: 100) */
  pageSize?: number;
}

/**
 * Search result from fuzzy directory search
 */
export interface DirSearchResult {
  /** Matched directory items */
  items: DirItem[];
  /** Corresponding scores for each item */
  scores: Score[];
  /** Total number of directories that matched */
  totalMatched: number;
  /** Total number of indexed directories */
  totalDirs: number;
}

/**
 * A single item in a mixed (files + directories) search result
 */
export type MixedItem =
  | { type: "file"; item: FileItem }
  | { type: "directory"; item: DirItem };

/**
 * Search result from mixed (files + directories) fuzzy search.
 * Items are interleaved by total score in descending order.
 */
export interface MixedSearchResult {
  /** Matched items (files and directories interleaved by score) */
  items: MixedItem[];
  /** Corresponding scores for each item */
  scores: Score[];
  /** Total number of items (files + dirs) that matched */
  totalMatched: number;
  /** Total number of indexed files */
  totalFiles: number;
  /** Total number of indexed directories */
  totalDirs: number;
  /** Location parsed from query */
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
  /** Whether the background file watcher is ready */
  isWatcherReady: boolean;
  /** Whether the warmup/bigram phase has completed */
  isWarmupComplete: boolean;
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
 * accessed first). The engine returns a `nextCursor` for fetching the next page.
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
  /** Search mode (default: "plain") */
  mode?: GrepMode;
  /**
   * Maximum wall-clock time in milliseconds to spend searching before returning
   * partial results. 0 = unlimited. (default: 0)
   */
  timeBudgetMs?: number;
  /** Number of context lines to include before each match (default: 0) */
  beforeContext?: number;
  /** Number of context lines to include after each match (default: 0) */
  afterContext?: number;
}

/**
 * A single grep match with file and line information
 */
export interface GrepMatch {
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
  /** Lines before the match (context). Empty array when context is 0. */
  contextBefore?: string[];
  /** Lines after the match (context). Empty array when context is 0. */
  contextAfter?: string[];
}

/**
 * Result from a grep search
 */
export interface GrepResult {
  /** Matched items with file and line information. At most `max_matches_per_file`. */
  items: GrepMatch[];
  /** Total number of matches collected (always equal to items.length). */
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
 * Options for multi-pattern grep (Aho-Corasick multi-needle search)
 *
 * Searches for lines matching ANY of the provided patterns using
 * SIMD-accelerated Aho-Corasick multi-pattern matching.
 */
export interface MultiGrepOptions {
  /** Patterns to search for (OR logic — matches lines containing any pattern) */
  patterns: string[];
  /** File constraints like "*.rs" or "/src/" */
  constraints?: string;
  /** Maximum file size to search in bytes (default: 10MB) */
  maxFileSize?: number;
  /** Maximum matching lines to collect from a single file (default: 0 = unlimited) */
  maxMatchesPerFile?: number;
  /** Smart case: case-insensitive when all patterns are lowercase (default: true) */
  smartCase?: boolean;
  /**
   * Pagination cursor from a previous `GrepResult.nextCursor`.
   * Omit (or pass `null`) for the first page.
   */
  cursor?: GrepCursor | null;
  /**
   * Maximum wall-clock time in milliseconds to spend searching before returning
   * partial results. 0 = unlimited. (default: 0)
   */
  timeBudgetMs?: number;
  /** Number of context lines to include before each match (default: 0) */
  beforeContext?: number;
  /** Number of context lines to include after each match (default: 0) */
  afterContext?: number;
}
