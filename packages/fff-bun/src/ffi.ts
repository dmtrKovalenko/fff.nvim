/**
 * Bun FFI bindings for the fff-c native library
 *
 * This module uses Bun's native FFI to call into the Rust C library.
 * All functions follow the Result pattern for error handling.
 *
 * The API is instance-based: `ffiCreate` returns an opaque handle that must
 * be passed to all subsequent calls and freed with `ffiDestroy`.
 */

import { dlopen, FFIType, ptr, CString, read, type Pointer } from "bun:ffi";
import { findBinary, ensureBinary } from "./download";
import type { Result } from "./types";
import { err } from "./types";

// Define the FFI symbols
const ffiDefinition = {
  // Lifecycle
  fff_create: {
    args: [FFIType.cstring],
    returns: FFIType.ptr,
  },
  fff_destroy: {
    args: [FFIType.ptr],
    returns: FFIType.void,
  },

  // Search
  fff_search: {
    args: [FFIType.ptr, FFIType.cstring, FFIType.cstring],
    returns: FFIType.ptr,
  },

  // Live grep (content search)
  fff_live_grep: {
    args: [FFIType.ptr, FFIType.cstring, FFIType.cstring],
    returns: FFIType.ptr,
  },

  // File index
  fff_scan_files: {
    args: [FFIType.ptr],
    returns: FFIType.ptr,
  },
  fff_is_scanning: {
    args: [FFIType.ptr],
    returns: FFIType.bool,
  },
  fff_get_scan_progress: {
    args: [FFIType.ptr],
    returns: FFIType.ptr,
  },
  fff_wait_for_scan: {
    args: [FFIType.ptr, FFIType.u64],
    returns: FFIType.ptr,
  },
  fff_restart_index: {
    args: [FFIType.ptr, FFIType.cstring],
    returns: FFIType.ptr,
  },

  // Frecency
  fff_track_access: {
    args: [FFIType.ptr, FFIType.cstring],
    returns: FFIType.ptr,
  },

  // Git
  fff_refresh_git_status: {
    args: [FFIType.ptr],
    returns: FFIType.ptr,
  },

  // Query tracking
  fff_track_query: {
    args: [FFIType.ptr, FFIType.cstring, FFIType.cstring],
    returns: FFIType.ptr,
  },
  fff_get_historical_query: {
    args: [FFIType.ptr, FFIType.u64],
    returns: FFIType.ptr,
  },

  // Utilities
  fff_health_check: {
    args: [FFIType.ptr, FFIType.cstring],
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
 * Parse a FffResult from the FFI return value.
 *
 * The result is a pointer to a struct:
 *   { success: bool, data: *char, error: *char, handle: *void }
 *
 * Layout (with alignment padding):
 *   offset  0: success (bool, 1 byte + 7 padding)
 *   offset  8: data pointer (8 bytes)
 *   offset 16: error pointer (8 bytes)
 *   offset 24: handle pointer (8 bytes)
 */
function parseResult<T>(resultPtr: Pointer | null): Result<T> {
  if (resultPtr === null) {
    return err("FFI returned null pointer");
  }

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
 * Opaque native handle type. Callers must not inspect or modify this value.
 */
export type NativeHandle = Pointer;

/**
 * Create a new file finder instance.
 *
 * Returns the opaque native handle on success. The handle must be passed to
 * all subsequent FFI calls and freed with `ffiDestroy`.
 */
export function ffiCreate(optsJson: string): Result<NativeHandle> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_create(ptr(encodeString(optsJson)));

  if (resultPtr === null) {
    return err("FFI returned null pointer");
  }

  const success = read.u8(resultPtr, 0) !== 0;
  const errorPtr = read.ptr(resultPtr, 16);
  const handlePtr = read.ptr(resultPtr, 24);

  if (success) {
    const handle = handlePtr as unknown as Pointer;
    library.symbols.fff_free_result(resultPtr);

    if (!handle || handle === (0 as unknown as Pointer)) {
      return err("fff_create returned null handle");
    }

    return { ok: true, value: handle };
  } else {
    const errorMsg = readCString(errorPtr) || "Unknown error";
    library.symbols.fff_free_result(resultPtr);
    return err(errorMsg);
  }
}

/**
 * Destroy and clean up an instance.
 */
export function ffiDestroy(handle: NativeHandle): void {
  const library = loadLibrary();
  library.symbols.fff_destroy(handle);
}

/**
 * Perform fuzzy search.
 */
export function ffiSearch(
  handle: NativeHandle,
  query: string,
  optsJson: string
): Result<unknown> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_search(
    handle,
    ptr(encodeString(query)),
    ptr(encodeString(optsJson))
  );
  return parseResult<unknown>(resultPtr);
}

/**
 * Trigger file scan.
 */
export function ffiScanFiles(handle: NativeHandle): Result<void> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_scan_files(handle);
  return parseResult<void>(resultPtr);
}

/**
 * Check if scanning.
 */
export function ffiIsScanning(handle: NativeHandle): boolean {
  const library = loadLibrary();
  return library.symbols.fff_is_scanning(handle) as boolean;
}

/**
 * Get scan progress.
 */
export function ffiGetScanProgress(handle: NativeHandle): Result<unknown> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_get_scan_progress(handle);
  return parseResult<unknown>(resultPtr);
}

/**
 * Wait for scan to complete.
 */
export function ffiWaitForScan(
  handle: NativeHandle,
  timeoutMs: number
): Result<boolean> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_wait_for_scan(
    handle,
    BigInt(timeoutMs)
  );
  const result = parseResult<string>(resultPtr);
  if (!result.ok) return result;
  return { ok: true, value: result.value === "true" };
}

/**
 * Restart index in new path.
 */
export function ffiRestartIndex(
  handle: NativeHandle,
  newPath: string
): Result<void> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_restart_index(
    handle,
    ptr(encodeString(newPath))
  );
  return parseResult<void>(resultPtr);
}

/**
 * Track file access.
 */
export function ffiTrackAccess(
  handle: NativeHandle,
  filePath: string
): Result<boolean> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_track_access(
    handle,
    ptr(encodeString(filePath))
  );
  const result = parseResult<string>(resultPtr);
  if (!result.ok) return result;
  return { ok: true, value: result.value === "true" };
}

/**
 * Refresh git status.
 */
export function ffiRefreshGitStatus(handle: NativeHandle): Result<number> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_refresh_git_status(handle);
  const result = parseResult<string>(resultPtr);
  if (!result.ok) return result;
  return { ok: true, value: parseInt(result.value, 10) };
}

/**
 * Track query completion.
 */
export function ffiTrackQuery(
  handle: NativeHandle,
  query: string,
  filePath: string
): Result<boolean> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_track_query(
    handle,
    ptr(encodeString(query)),
    ptr(encodeString(filePath))
  );
  const result = parseResult<string>(resultPtr);
  if (!result.ok) return result;
  return { ok: true, value: result.value === "true" };
}

/**
 * Get historical query.
 */
export function ffiGetHistoricalQuery(
  handle: NativeHandle,
  offset: number
): Result<string | null> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_get_historical_query(
    handle,
    BigInt(offset)
  );
  const result = parseResult<string>(resultPtr);
  if (!result.ok) return result;
  if (result.value === "null") return { ok: true, value: null };
  return result;
}

/**
 * Health check.
 *
 * `handle` can be null for a limited check (version + git only).
 */
export function ffiHealthCheck(
  handle: NativeHandle | null,
  testPath: string
): Result<unknown> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_health_check(
    handle ?? (0 as unknown as Pointer),
    ptr(encodeString(testPath))
  );
  return parseResult<unknown>(resultPtr);
}

/**
 * Live grep - search file contents.
 */
export function ffiLiveGrep(
  handle: NativeHandle,
  query: string,
  optsJson: string
): Result<unknown> {
  const library = loadLibrary();
  const resultPtr = library.symbols.fff_live_grep(
    handle,
    ptr(encodeString(query)),
    ptr(encodeString(optsJson))
  );
  return parseResult<unknown>(resultPtr);
}

/**
 * Ensure the library is loaded (for preloading).
 */
export async function ensureLoaded(): Promise<void> {
  await ensureBinary();
  loadLibrary();
}

/**
 * Check if the library is available.
 */
export function isAvailable(): boolean {
  try {
    loadLibrary();
    return true;
  } catch {
    return false;
  }
}
