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
  /** Path to frecency database (optional, defaults to ~/.fff/frecency.mdb) */
  frecencyDbPath?: string;
  /** Path to query history database (optional, defaults to ~/.fff/history.mdb) */
  historyDbPath?: string;
  /** Use unsafe no-lock mode for databases (optional, defaults to false) */
  useUnsafeNoLock?: boolean;
  /** Skip database initialization entirely (optional, defaults to false) */
  skipDatabases?: boolean;
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
  skip_databases: boolean;
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
    skip_databases: opts.skipDatabases ?? false,
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
