import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import { FileFinder } from "./index";
import { findBinary, getDevBinaryPath } from "./download";
import { getTriple, getLibExtension, getLibFilename } from "./platform";

// Use a single shared instance to avoid thread cleanup issues
const testDir = process.cwd();

describe("Platform Detection", () => {
  test("getTriple returns valid triple", () => {
    const triple = getTriple();
    expect(triple).toMatch(
      /^(x86_64|aarch64|arm)-(apple-darwin|unknown-linux-(gnu|musl)|pc-windows-msvc)$/,
    );
  });

  test("getLibExtension returns correct extension", () => {
    const ext = getLibExtension();
    const platform = process.platform;

    if (platform === "darwin") {
      expect(ext).toBe("dylib");
    } else if (platform === "win32") {
      expect(ext).toBe("dll");
    } else {
      expect(ext).toBe("so");
    }
  });

  test("getLibFilename returns correct filename", () => {
    const filename = getLibFilename();
    const ext = getLibExtension();

    if (process.platform === "win32") {
      expect(filename).toBe(`fff_c.${ext}`);
    } else {
      expect(filename).toBe(`libfff_c.${ext}`);
    }
  });
});

describe("Binary Detection", () => {
  test("getDevBinaryPath finds local build", () => {
    const devPath = getDevBinaryPath();
    expect(devPath).not.toBeNull();
    expect(devPath).toContain("target/release");
  });

  test("findBinary returns a path", () => {
    const path = findBinary();
    expect(path).not.toBeNull();
  });
});

describe("FileFinder - Availability", () => {
  test("isAvailable returns true when binary exists", () => {
    const available = FileFinder.isAvailable();
    expect(available).toBe(true);
  });
});

describe("FileFinder - Health Check (before init)", () => {
  test("healthCheck works before initialization", () => {
    // Make sure we start fresh
    FileFinder.destroy();

    const result = FileFinder.healthCheck();
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.version).toBeDefined();
      expect(result.value.git.available).toBe(true);
      expect(result.value.filePicker.initialized).toBe(false);
    }
  });
});

describe("FileFinder - Full Lifecycle", () => {
  // Single beforeAll/afterAll for the entire test suite to avoid repeated init/destroy
  beforeAll(() => {
    FileFinder.destroy(); // Clean any previous state
  });

  afterAll(() => {
    FileFinder.destroy();
  });

  test("init succeeds with valid path", () => {
    const result = FileFinder.init({
      basePath: testDir,
      skipDatabases: true,
    });

    expect(result.ok).toBe(true);
    expect(FileFinder.isInitialized()).toBe(true);
  });

  test("isScanning returns a boolean", () => {
    const scanning = FileFinder.isScanning();
    expect(typeof scanning).toBe("boolean");
  });

  test("getScanProgress returns valid data", () => {
    const result = FileFinder.getScanProgress();
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(typeof result.value.scannedFilesCount).toBe("number");
      expect(typeof result.value.isScanning).toBe("boolean");
    }
  });

  test("waitForScan completes", () => {
    // Small timeout - scan should be fast or already done
    const result = FileFinder.waitForScan(500);
    expect(result.ok).toBe(true);
  });

  test("search with empty query returns all files", () => {
    const result = FileFinder.search("");
    expect(result.ok).toBe(true);
    
    if (result.ok) {
      // Empty query should return files (frecency-sorted)
      expect(result.value.totalFiles).toBeGreaterThan(0);
    }
  });

  test("search returns a valid result structure", () => {
    const result = FileFinder.search("Cargo.toml");
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(typeof result.value.totalMatched).toBe("number");
      expect(typeof result.value.totalFiles).toBe("number");
      expect(Array.isArray(result.value.items)).toBe(true);
      expect(Array.isArray(result.value.scores)).toBe(true);
    }
  });

  test("search returns empty for non-matching query", () => {
    const result = FileFinder.search("xyznonexistentfilenamexyz123456");
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.totalMatched).toBe(0);
      expect(result.value.items.length).toBe(0);
    }
  });

  test("search respects pageSize option", () => {
    const result = FileFinder.search("ts", { pageSize: 3 });
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.items.length).toBeLessThanOrEqual(3);
    }
  });

  test("healthCheck shows initialized state", () => {
    const result = FileFinder.healthCheck();
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.filePicker.initialized).toBe(true);
      expect(result.value.filePicker.basePath).toBeDefined();
      expect(typeof result.value.filePicker.indexedFiles).toBe("number");
    }
  });

  test("healthCheck detects git repository", () => {
    const result = FileFinder.healthCheck(testDir);
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.git.available).toBe(true);
      expect(typeof result.value.git.repositoryFound).toBe("boolean");
    }
  });

  test("destroy and re-init works", () => {
    FileFinder.destroy();
    expect(FileFinder.isInitialized()).toBe(false);

    const result = FileFinder.init({
      basePath: testDir,
      skipDatabases: true,
    });
    expect(result.ok).toBe(true);
    expect(FileFinder.isInitialized()).toBe(true);
  });
});

describe("FileFinder - Utilities (stateless)", () => {
  test("shortenPath shortens long paths", () => {
    const longPath = "/very/long/path/to/some/deeply/nested/file.ts";
    const result = FileFinder.shortenPath(longPath, 20, "middle_number");

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.value.length).toBeLessThanOrEqual(25);
    }
  });

  test("shortenPath handles short paths", () => {
    const shortPath = "file.ts";
    const result = FileFinder.shortenPath(shortPath, 50, "middle_number");

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.value).toBe(shortPath);
    }
  });
});

describe("FileFinder - Error Handling", () => {
  test("search fails when not initialized", () => {
    FileFinder.destroy();

    const result = FileFinder.search("test");
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.error).toContain("not initialized");
    }
  });

  test("getScanProgress fails when not initialized", () => {
    const result = FileFinder.getScanProgress();
    expect(result.ok).toBe(false);
  });

  test("init fails with invalid path", () => {
    const result = FileFinder.init({
      basePath: "/nonexistent/path/that/does/not/exist",
      skipDatabases: true,
    });

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.error).toContain("Failed");
    }
  });
});

describe("Result Type Helpers", () => {
  test("ok helper creates success result", async () => {
    const { ok } = await import("./types");
    const result = ok(42);
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.value).toBe(42);
    }
  });

  test("err helper creates error result", async () => {
    const { err } = await import("./types");
    const result = err<number>("something went wrong");
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.error).toBe("something went wrong");
    }
  });
});
