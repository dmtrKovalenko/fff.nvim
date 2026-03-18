/**
 * Node.js FFI bindings for the fff-c native library using ffi-rs
 *
 * This module uses ffi-rs to call into the Rust C library.
 * All functions follow the Result pattern for error handling.
 *
 * The API is instance-based: `ffiCreate` returns an opaque handle that must
 * be passed to all subsequent calls and freed with `ffiDestroy`.
 *
 * ## Memory management
 *
 * Every `fff_*` function returning `*mut FffResult` allocates with Rust's Box.
 * We MUST call `fff_free_result` to properly deallocate (not libc::free).
 *
 * ## FffResult struct reading
 *
 * The FffResult struct layout (#[repr(C)]):
 *   offset  0: success (bool, 1 byte + 7 padding)
 *   offset  8: data pointer (8 bytes) - *mut c_char (JSON string or null)
 *   offset 16: error pointer (8 bytes) - *mut c_char (error message or null)
 *   offset 24: handle pointer (8 bytes) - *mut c_void (instance handle or null)
 *
 * ## Two-step approach for reading + freeing
 *
 * ffi-rs auto-dereferences struct retType pointers, losing the original pointer.
 * We solve this by:
 * 1. Calling the C function with `retType: DataType.External` to get the raw pointer
 * 2. Using `restorePointer` to read the struct fields from the raw pointer
 * 3. Calling `fff_free_result` with the original raw pointer
 *
 * ## Null pointer detection
 *
 * `isNullPointer` from ffi-rs correctly detects null C pointers wrapped as
 * V8 External objects. We use this instead of truthy checks.
 */

import {
  close,
  DataType,
  isNullPointer,
  type JsExternal,
  load,
  open,
  restorePointer,
  wrapPointer,
} from "ffi-rs";
import { findBinary } from "./binary.js";
import type { Result } from "./types.js";
import { err } from "./types.js";

const LIBRARY_KEY = "fff_c";

// Track whether the library is loaded
let isLoaded = false;

/**
 * Struct type definition for FffResult used with restorePointer.
 *
 * Uses U8 for the bool success field (correct alignment with ffi-rs).
 * Uses External for ALL pointer fields to avoid hangs on null char* pointers
 * (ffi-rs hangs when trying to read DataType.String from null char*).
 */
const FFF_RESULT_STRUCT = {
  success: DataType.U8,
  data: DataType.External,
  error: DataType.External,
  handle: DataType.External,
};

interface FffResultRaw {
  success: number;
  data: JsExternal;
  error: JsExternal;
  handle: JsExternal;
}

/**
 * Load the native library using ffi-rs
 */
function loadLibrary(): void {
  if (isLoaded) return;

  const binaryPath = findBinary();
  if (!binaryPath) {
    throw new Error(
      "fff native library not found. Run `npx @ff-labs/fff-node download` or build from source with `cargo build --release -p fff-c`",
    );
  }

  open({ library: LIBRARY_KEY, path: binaryPath });
  isLoaded = true;
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
    const camelKey = key.replace(/_([a-z])/g, (_, letter: string) =>
      letter.toUpperCase(),
    );
    result[camelKey] = snakeToCamel(value);
  }
  return result;
}

/**
 * Read a C string (char*) from an ffi-rs External pointer.
 *
 * Uses restorePointer + wrapPointer to dereference the char* and read the
 * null-terminated string. Returns null if the pointer is null.
 */
function readCString(ptr: JsExternal): string | null {
  if (isNullPointer(ptr)) return null;
  try {
    const [str] = restorePointer({
      retType: [DataType.String],
      paramsValue: wrapPointer([ptr]),
    });
    return str as string;
  } catch {
    return null;
  }
}

/**
 * Call a C function that returns `*mut FffResult` and get both the raw pointer
 * (for freeing) and the parsed struct fields.
 *
 * Step 1: Call function with `DataType.External` retType → raw pointer
 * Step 2: Use `restorePointer` to read struct fields from the raw pointer
 */
function callRaw(
  funcName: string,
  paramsType: DataType[],
  paramsValue: unknown[],
): { rawPtr: JsExternal; struct: FffResultRaw } {
  const rawPtr = load({
    library: LIBRARY_KEY,
    funcName,
    retType: DataType.External,
    paramsType,
    paramsValue,
    freeResultMemory: false,
  }) as JsExternal;

  const [structData] = restorePointer({
    retType: [FFF_RESULT_STRUCT],
    paramsValue: wrapPointer([rawPtr]),
  }) as unknown as [FffResultRaw];

  return { rawPtr, struct: structData };
}

/**
 * Free a FffResult pointer by calling fff_free_result.
 *
 * This frees the FffResult struct and its data/error strings using Rust's
 * Box::from_raw and CString::from_raw. The handle field is NOT freed.
 */
function freeResult(resultPtr: JsExternal): void {
  try {
    load({
      library: LIBRARY_KEY,
      funcName: "fff_free_result",
      retType: DataType.Void,
      paramsType: [DataType.External],
      paramsValue: [resultPtr],
    });
  } catch {
    // Ignore cleanup errors
  }
}

/**
 * Call a fff C function that returns *mut FffResult, parse the result,
 * free the native memory, and return a typed Result<T>.
 *
 * Strategy:
 * 1. Call the C function with External retType to get the raw pointer
 * 2. Read struct fields via restorePointer
 * 3. Based on success flag, read the data or error string
 * 4. Call fff_free_result with the original raw pointer to free Rust memory
 * 5. Parse JSON data and convert snake_case to camelCase
 */
function callFfiResult<T>(
  funcName: string,
  paramsType: DataType[],
  paramsValue: unknown[],
): Result<T> {
  loadLibrary();

  const { rawPtr, struct: structData } = callRaw(funcName, paramsType, paramsValue);

  const success = structData.success !== 0;

  try {
    if (success) {
      const dataStr = readCString(structData.data);

      if (dataStr === null || dataStr === "") {
        return { ok: true, value: undefined as T };
      }

      try {
        const parsed = JSON.parse(dataStr);
        const transformed = snakeToCamel(parsed) as T;
        return { ok: true, value: transformed };
      } catch {
        // For simple values like "true" or numbers
        return { ok: true, value: dataStr as T };
      }
    } else {
      const errorStr = readCString(structData.error);
      return err(errorStr || "Unknown error");
    }
  } finally {
    freeResult(rawPtr);
  }
}

/**
 * Opaque native handle type. Callers must not inspect or modify this value.
 */
export type NativeHandle = JsExternal;

/**
 * Create a new file finder instance.
 *
 * Returns the opaque native handle on success. The handle must be passed to
 * all subsequent FFI calls and freed with `ffiDestroy`.
 */
export function ffiCreate(optsJson: string): Result<NativeHandle> {
  loadLibrary();

  const { rawPtr, struct: structData } = callRaw(
    "fff_create",
    [DataType.String],
    [optsJson],
  );

  const success = structData.success !== 0;

  try {
    if (success) {
      const handle = structData.handle;
      if (isNullPointer(handle)) {
        return err("fff_create returned null handle");
      }
      return { ok: true, value: handle };
    } else {
      const errorStr = readCString(structData.error);
      return err(errorStr || "Unknown error");
    }
  } finally {
    freeResult(rawPtr);
  }
}

/**
 * Destroy and clean up an instance.
 */
export function ffiDestroy(handle: NativeHandle): void {
  loadLibrary();
  load({
    library: LIBRARY_KEY,
    funcName: "fff_destroy",
    retType: DataType.Void,
    paramsType: [DataType.External],
    paramsValue: [handle],
  });
}

/**
 * Perform fuzzy search.
 */
export function ffiSearch(
  handle: NativeHandle,
  query: string,
  optsJson: string,
): Result<unknown> {
  return callFfiResult<unknown>(
    "fff_search",
    [DataType.External, DataType.String, DataType.String],
    [handle, query, optsJson],
  );
}

/**
 * Trigger file scan.
 */
export function ffiScanFiles(handle: NativeHandle): Result<void> {
  return callFfiResult<void>("fff_scan_files", [DataType.External], [handle]);
}

/**
 * Check if scanning.
 */
export function ffiIsScanning(handle: NativeHandle): boolean {
  loadLibrary();
  return load({
    library: LIBRARY_KEY,
    funcName: "fff_is_scanning",
    retType: DataType.Boolean,
    paramsType: [DataType.External],
    paramsValue: [handle],
  }) as boolean;
}

/**
 * Get scan progress.
 */
export function ffiGetScanProgress(handle: NativeHandle): Result<unknown> {
  return callFfiResult<unknown>("fff_get_scan_progress", [DataType.External], [handle]);
}

/**
 * Wait for a tree scan to complete.
 */
export function ffiWaitForScan(handle: NativeHandle, timeoutMs: number): Result<boolean> {
  const result = callFfiResult<boolean | string>(
    "fff_wait_for_scan",
    [DataType.External, DataType.U64],
    [handle, timeoutMs],
  );
  if (!result.ok) return result;
  return { ok: true, value: result.value === true || result.value === "true" };
}

/**
 * Restart index in new path.
 */
export function ffiRestartIndex(handle: NativeHandle, newPath: string): Result<void> {
  return callFfiResult<void>(
    "fff_restart_index",
    [DataType.External, DataType.String],
    [handle, newPath],
  );
}

/**
 * Refresh git status.
 */
export function ffiRefreshGitStatus(handle: NativeHandle): Result<number> {
  const result = callFfiResult<number | string>(
    "fff_refresh_git_status",
    [DataType.External],
    [handle],
  );
  if (!result.ok) return result;
  return {
    ok: true,
    value:
      typeof result.value === "number"
        ? result.value
        : parseInt(result.value as string, 10),
  };
}

/**
 * Track query completion.
 */
export function ffiTrackQuery(
  handle: NativeHandle,
  query: string,
  filePath: string,
): Result<boolean> {
  const result = callFfiResult<boolean | string>(
    "fff_track_query",
    [DataType.External, DataType.String, DataType.String],
    [handle, query, filePath],
  );
  if (!result.ok) return result;
  return { ok: true, value: result.value === true || result.value === "true" };
}

/**
 * Get historical query.
 */
export function ffiGetHistoricalQuery(
  handle: NativeHandle,
  offset: number,
): Result<string | null> {
  const result = callFfiResult<string | null>(
    "fff_get_historical_query",
    [DataType.External, DataType.U64],
    [handle, offset],
  );
  if (!result.ok) return result;
  if (result.value === null || result.value === "null") return { ok: true, value: null };
  return result as Result<string>;
}

/**
 * Health check.
 *
 * `handle` can be null for a limited check (version + git only).
 * When null, we pass DataType.U64 with value 0 as a null pointer workaround
 * since ffi-rs does not accept `null` for External parameters.
 */
export function ffiHealthCheck(
  handle: NativeHandle | null,
  testPath: string,
): Result<unknown> {
  loadLibrary();

  if (handle === null) {
    // Use U64(0) as a null pointer since ffi-rs rejects null for External params
    const rawPtr = load({
      library: LIBRARY_KEY,
      funcName: "fff_health_check",
      retType: DataType.External,
      paramsType: [DataType.U64, DataType.String],
      paramsValue: [0, testPath],
      freeResultMemory: false,
    }) as JsExternal;

    const [structData] = restorePointer({
      retType: [FFF_RESULT_STRUCT],
      paramsValue: wrapPointer([rawPtr]),
    }) as unknown as [FffResultRaw];

    const success = structData.success !== 0;

    try {
      if (success) {
        const dataStr = readCString(structData.data);
        if (dataStr === null || dataStr === "") {
          return { ok: true, value: undefined as unknown };
        }
        try {
          return { ok: true, value: snakeToCamel(JSON.parse(dataStr)) };
        } catch {
          return { ok: true, value: dataStr };
        }
      } else {
        const errorStr = readCString(structData.error);
        return err(errorStr || "Unknown error");
      }
    } finally {
      freeResult(rawPtr);
    }
  }

  return callFfiResult<unknown>(
    "fff_health_check",
    [DataType.External, DataType.String],
    [handle, testPath],
  );
}

/**
 * Live grep - search file contents.
 */
export function ffiLiveGrep(
  handle: NativeHandle,
  query: string,
  optsJson: string,
): Result<unknown> {
  return callFfiResult<unknown>(
    "fff_live_grep",
    [DataType.External, DataType.String, DataType.String],
    [handle, query, optsJson],
  );
}

/**
 * Multi-pattern grep - Aho-Corasick multi-needle search.
 */
export function ffiMultiGrep(handle: NativeHandle, optsJson: string): Result<unknown> {
  return callFfiResult<unknown>(
    "fff_multi_grep",
    [DataType.External, DataType.String],
    [handle, optsJson],
  );
}

/**
 * Ensure the library is loaded.
 *
 * Loads the native library from the platform-specific npm package
 * or a local dev build. Throws if the binary is not found.
 */
export function ensureLoaded(): void {
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

/**
 * Close the library and release ffi-rs resources.
 * Call this when completely done with the library.
 */
export function closeLibrary(): void {
  if (isLoaded) {
    close(LIBRARY_KEY);
    isLoaded = false;
  }
}
