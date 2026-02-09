/**
 * Bun FFI bindings for the fff-c native library
 *
 * This module uses Bun's native FFI to call into the Rust C library.
 * All functions follow the Result pattern for error handling.
 */

import { dlopen, FFIType, ptr, CString, read, type Pointer } from "bun:ffi";
import { findBinary, ensureBinary } from "./download";
import type { Result } from "./types";
import { err } from "./types";

// Define the FFI symbols
const ffiDefinition = {
  // Lifecycle
  fff_init: {
    args: [FFIType.cstring],
    returns: FFIType.ptr,
  },
  fff_destroy: {
    args: [],
    returns: FFIType.ptr,
  },

  // Search
  fff_search: {
    args: [FFIType.cstring, FFIType.cstring],
    returns: FFIType.ptr,
  },

  // File index
  fff_scan_files: {
    args: [],
    returns: FFIType.ptr,
  },
  fff_is_scanning: {
    args: [],
    returns: FFIType.bool,
  },
  fff_get_scan_progress: {
    args: [],
    returns: FFIType.ptr,
  },
  fff_wait_for_scan: {
    args: [FFIType.u64],
    returns: FFIType.ptr,
  },
  fff_restart_index: {
    args: [FFIType.cstring],
    returns: FFIType.ptr,
  },

  // Frecency
  fff_track_access: {
    args: [FFIType.cstring],
    returns: FFIType.ptr,
  },

  // Git
  fff_refresh_git_status: {
    args: [],
    returns: FFIType.ptr,
  },

  // Query tracking
  fff_track_query: {
    args: [FFIType.cstring, FFIType.cstring],
    returns: FFIType.ptr,
  },
  fff_get_historical_query: {
    args: [FFIType.u64],
    returns: FFIType.ptr,
  },

  // Utilities
  fff_health_check: {
    args: [FFIType.cstring],
    returns: FFIType.ptr,
  },
  fff_shorten_path: {
    args: [FFIType.cstring, FFIType.u64, FFIType.cstring],
    returns: FFIType.ptr,
  },

  // Memory management
  fff_free_result: {
    args: [FFIType.ptr],
    returns: FFIType.void,
  },
  fff_free_string: {
    args: [FFIType.ptr],
    returns: FFIType.void,
  },
} as const;

type FFFLibrary = ReturnType<typeof dlopen<typeof ffiDefinition>>;

// Library instance (lazy loaded)
let lib: FFFLibrary | null = null;

/**
 * Load the native library
 */
function loadLibrary(): FFFLibrary {
  if (lib) return lib;

  const binaryPath = findBinary();
  if (!binaryPath) {
    throw new Error(
      "fff native library not found. Run `bunx fff download` or build from source with `cargo build --release -p fff-c`"
    );
  }

  lib = dlopen(binaryPath, ffiDefinition);
  return lib;
}

/**
 * Encode a string for FFI (null-terminated)
 */
function encodeString(s: string): Uint8Array {
  return new TextEncoder().encode(s + "\0");
}

/**
 * Read a C string from a pointer
 * Note: read.ptr() returns number but CString expects Pointer - we cast through unknown
 */
function readCString(pointer: Pointer | number | null): string | null {
  if (pointer === null || pointer === 0) return null;
  // CString constructor accepts Pointer, but read.ptr returns number
  // Cast through unknown for runtime compatibility
  return new CString(pointer as unknown as Pointer).toString();
}

/**
 * Convert snake_case keys to camelCase recursively
 */
function snakeToCamel(obj: unknown): unknown {
  if (obj === null || obj === undefined) return obj;
  if (typeof obj !== "object") return obj;
  if (Array.isArray(obj)) return obj.map(snakeToCamel);

  const result: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(obj as Record<string, unknown>)) {
    const camelKey = key.replace(/_([a-z])/g, (_, letter) =>
      letter.toUpperCase()
    );
    result[camelKey] = snakeToCamel(value);
  }
  return result;
}

/**
 * Parse a FffResult from the FFI return value
 * The result is a pointer to a struct: { success: bool, data: *char, error: *char }
 */
function parseResult<T>(resultPtr: Pointer | null): Result<T> {
  if (resultPtr === null) {
    return err("FFI returned null pointer");
  }

  // Read the struct fields
  // FffResult layout: bool (1 byte + 7 padding) + pointer (8 bytes) + pointer (8 bytes)
  // offset 0: success (bool, 1 byte)
  // offset 8: data pointer (8 bytes)
  // offset 16: error pointer (8 bytes)
  const success = read.u8(resultPtr, 0) !== 0;
  const dataPtr = read.ptr(resultPtr, 8);
  const errorPtr = read.ptr(resultPtr, 16);

  const library = loadLibrary();

  if (success) {
    const data = readCString(dataPtr);
    // Free the result
    library.symbols.fff_free_result(resultPtr);

    if (data === null || data === "") {
      return { ok: true, value: undefined as T };
    }

    try {
      const parsed = JSON.parse(data);
      // Convert snake_case to camelCase for TypeScript consumers
      const transformed = snakeToCamel(parsed) as T;
      return { ok: true, value: transformed };
    } catch {
      // For simple values like "true" or numbers
      return { ok: true, value: data as T };
    }
  } else {
    const errorMsg = readCString(errorPtr) || "Unknown error";
    // Free the result
    library.symbols.fff_free_result(resultPtr);
    return err(errorMsg);
  }
}

/**
 * Initialize the file finder
 */
export function ffiInit(optsJson: string): Result<void> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_init(ptr(encodeString(optsJson)));
  return parseResult<void>(resultPtr);
}

/**
 * Destroy and clean up resources
 */
export function ffiDestroy(): Result<void> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_destroy();
  return parseResult<void>(resultPtr);
}

/**
 * Perform fuzzy search
 */
export function ffiSearch(query: string, optsJson: string): Result<unknown> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_search(
    ptr(encodeString(query)),
    ptr(encodeString(optsJson))
  );
  return parseResult<unknown>(resultPtr);
}

/**
 * Trigger file scan
 */
export function ffiScanFiles(): Result<void> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_scan_files();
  return parseResult<void>(resultPtr);
}

/**
 * Check if scanning
 */
export function ffiIsScanning(): boolean {
  const library = loadLibrary();
  return library.symbols.fff_is_scanning() as boolean;
}

/**
 * Get scan progress
 */
export function ffiGetScanProgress(): Result<unknown> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_get_scan_progress();
  return parseResult<unknown>(resultPtr);
}

/**
 * Wait for scan to complete
 */
export function ffiWaitForScan(timeoutMs: number): Result<boolean> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_wait_for_scan(BigInt(timeoutMs));
  const result = parseResult<string>(resultPtr);
  if (!result.ok) return result;
  return { ok: true, value: result.value === "true" };
}

/**
 * Restart index in new path
 */
export function ffiRestartIndex(newPath: string): Result<void> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_restart_index(
    ptr(encodeString(newPath))
  );
  return parseResult<void>(resultPtr);
}

/**
 * Track file access
 */
export function ffiTrackAccess(filePath: string): Result<boolean> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_track_access(
    ptr(encodeString(filePath))
  );
  const result = parseResult<string>(resultPtr);
  if (!result.ok) return result;
  return { ok: true, value: result.value === "true" };
}

/**
 * Refresh git status
 */
export function ffiRefreshGitStatus(): Result<number> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_refresh_git_status();
  const result = parseResult<string>(resultPtr);
  if (!result.ok) return result;
  return { ok: true, value: parseInt(result.value, 10) };
}

/**
 * Track query completion
 */
export function ffiTrackQuery(
  query: string,
  filePath: string
): Result<boolean> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_track_query(
    ptr(encodeString(query)),
    ptr(encodeString(filePath))
  );
  const result = parseResult<string>(resultPtr);
  if (!result.ok) return result;
  return { ok: true, value: result.value === "true" };
}

/**
 * Get historical query
 */
export function ffiGetHistoricalQuery(offset: number): Result<string | null> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_get_historical_query(BigInt(offset));
  const result = parseResult<string>(resultPtr);
  if (!result.ok) return result;
  if (result.value === "null") return { ok: true, value: null };
  return result;
}

/**
 * Health check
 */
export function ffiHealthCheck(testPath: string): Result<unknown> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_health_check(
    ptr(encodeString(testPath))
  );
  return parseResult<unknown>(resultPtr);
}

/**
 * Shorten path
 */
export function ffiShortenPath(
  path: string,
  maxSize: number,
  strategy: string
): Result<string> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_shorten_path(
    ptr(encodeString(path)),
    BigInt(maxSize),
    ptr(encodeString(strategy))
  );
  return parseResult<string>(resultPtr);
}

/**
 * Ensure the library is loaded (for preloading)
 */
export async function ensureLoaded(): Promise<void> {
  await ensureBinary();
  loadLibrary();
}

/**
 * Check if the library is available
 */
export function isAvailable(): boolean {
  try {
    loadLibrary();
    return true;
  } catch {
    return false;
  }
}
