import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import { FileFinder } from "./index";
import { findBinary, getDevBinaryPath } from "./download";
import { getTriple, getLibExtension, getLibFilename } from "./platform";

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

describe("FileFinder - Health Check", () => {
  test("healthCheckStatic works without an instance", () => {
    const result = FileFinder.healthCheckStatic();
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.version).toBeDefined();
      expect(result.value.git.available).toBe(true);
      expect(result.value.filePicker.initialized).toBe(false);
    }
  });
});

describe("FileFinder - Full Lifecycle", () => {
  let finder: FileFinder;

  beforeAll(() => {
    const result = FileFinder.create({ basePath: testDir });
    expect(result.ok).toBe(true);
    if (result.ok) {
      finder = result.value;
    }
  });

  afterAll(() => {
    finder?.destroy();
  });

  test("create succeeds with valid path", () => {
    expect(finder).toBeDefined();
    expect(finder.isDestroyed).toBe(false);
  });

  test("isScanning returns a boolean", () => {
    const scanning = finder.isScanning();
    expect(typeof scanning).toBe("boolean");
  });

  test("getScanProgress returns valid data", () => {
    const result = finder.getScanProgress();
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(typeof result.value.scannedFilesCount).toBe("number");
      expect(typeof result.value.isScanning).toBe("boolean");
    }
  });

  test("waitForScan completes", () => {
    // Small timeout - scan should be fast or already done
    const result = finder.waitForScan(500);
    expect(result.ok).toBe(true);
  });

  test("search with empty query returns all files", () => {
    const result = finder.search("");
    expect(result.ok).toBe(true);

    if (result.ok) {
      // Empty query should return files (frecency-sorted)
      expect(result.value.totalFiles).toBeGreaterThan(0);
    }
  });

  test("search returns a valid result structure", () => {
    const result = finder.search("Cargo.toml");
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(typeof result.value.totalMatched).toBe("number");
      expect(typeof result.value.totalFiles).toBe("number");
      expect(Array.isArray(result.value.items)).toBe(true);
      expect(Array.isArray(result.value.scores)).toBe(true);
    }
  });

  test("search returns empty for non-matching query", () => {
    const result = finder.search("xyznonexistentfilenamexyz123456");
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.totalMatched).toBe(0);
      expect(result.value.items.length).toBe(0);
    }
  });

  test("search respects pageSize option", () => {
    const result = finder.search("ts", { pageSize: 3 });
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.items.length).toBeLessThanOrEqual(3);
    }
  });

  test("liveGrep plain text returns matching lines", () => {
    const result = finder.liveGrep("fff-core", { mode: "plain", pageLimit: 10 });
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.totalMatched).toBeGreaterThan(0);
      expect(result.value.items.length).toBeGreaterThan(0);
      expect(result.value.items.length).toBeLessThanOrEqual(10);

      const first = result.value.items[0];
      expect(typeof first.relativePath).toBe("string");
      expect(typeof first.lineNumber).toBe("number");
      expect(first.lineNumber).toBeGreaterThan(0);
      expect(typeof first.lineContent).toBe("string");
      expect(first.lineContent.toLowerCase()).toContain("fff-core");
      expect(Array.isArray(first.matchRanges)).toBe(true);
      expect(first.matchRanges.length).toBeGreaterThan(0);

      expect(typeof result.value.totalFilesSearched).toBe("number");
      expect(typeof result.value.totalFiles).toBe("number");
      expect(typeof result.value.filteredFileCount).toBe("number");
    }
  });

  test("liveGrep fuzzy mode returns results with scores", () => {
    // Intentional typo: "depdnency" instead of "dependency" to exercise fuzzy matching
    const result = finder.liveGrep("depdnency", { mode: "fuzzy", pageLimit: 10 });
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.totalMatched).toBeGreaterThan(0);
      expect(result.value.items.length).toBeGreaterThan(0);

      const first = result.value.items[0];
      expect(typeof first.relativePath).toBe("string");
      expect(typeof first.lineNumber).toBe("number");
      expect(typeof first.lineContent).toBe("string");
      // Fuzzy mode should produce a fuzzyScore on each match
      expect(typeof first.fuzzyScore).toBe("number");
    }
  });

  test("healthCheck shows initialized state", () => {
    const result = finder.healthCheck();
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.filePicker.initialized).toBe(true);
      expect(result.value.filePicker.basePath).toBeDefined();
      expect(typeof result.value.filePicker.indexedFiles).toBe("number");
    }
  });

  test("healthCheck detects git repository", () => {
    const result = finder.healthCheck(testDir);
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.git.available).toBe(true);
      expect(typeof result.value.git.repositoryFound).toBe("boolean");
    }
  });

  test("destroy and re-create works", () => {
    finder.destroy();
    expect(finder.isDestroyed).toBe(true);

    const result = FileFinder.create({ basePath: testDir });
    expect(result.ok).toBe(true);
    if (result.ok) {
      finder = result.value;
    }
    expect(finder.isDestroyed).toBe(false);
  });

  test("multiple instances can coexist", () => {
    const result2 = FileFinder.create({ basePath: testDir });
    expect(result2.ok).toBe(true);

    if (result2.ok) {
      const finder2 = result2.value;

      // Both should work independently
      const search1 = finder.search("Cargo");
      const search2 = finder2.search("Cargo");

      expect(search1.ok).toBe(true);
      expect(search2.ok).toBe(true);

      // Destroying one should not affect the other
      finder2.destroy();

      const search3 = finder.search("Cargo");
      expect(search3.ok).toBe(true);
    }
  });
});

describe("FileFinder - Error Handling", () => {
  test("search fails on destroyed instance", () => {
    const createResult = FileFinder.create({ basePath: testDir });
    expect(createResult.ok).toBe(true);
    if (!createResult.ok) return;

    const f = createResult.value;
    f.destroy();

    const result = f.search("test");
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.error).toContain("destroyed");
    }
  });

  test("getScanProgress fails on destroyed instance", () => {
    const createResult = FileFinder.create({ basePath: testDir });
    expect(createResult.ok).toBe(true);
    if (!createResult.ok) return;

    const f = createResult.value;
    f.destroy();

    const result = f.getScanProgress();
    expect(result.ok).toBe(false);
  });

  test("create fails with invalid path", () => {
    const result = FileFinder.create({
      basePath: "/nonexistent/path/that/does/not/exist",
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
