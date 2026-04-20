import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import fs from "node:fs";
import path from "node:path";
import { findBinary } from "./download";
import { FileFinder } from "./index";
import { getLibExtension, getLibFilename, getTriple } from "./platform";

// Cross-platform path normalization helpers
const normalizePath = (path: string | null | undefined): string | null => {
  if (!path) return null;
  // Convert backslashes to forward slashes for consistent comparison
  return path.replace(/\\/g, "/");
};

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
    // First check scan progress to see if files were indexed
    const progress = finder.getScanProgress();
    if (progress.ok) {
    }

    const result = finder.fileSearch("");
    expect(result.ok).toBe(true);

    if (result.ok) {
      if (result.value.items.length > 0) {
        // Log first few paths to see format on Windows
        // Items are strings (file paths), not objects
        const _samplePaths = result.value.items
          .slice(0, 3)
          .map((item) =>
            normalizePath(typeof item === "string" ? item : item.relativePath),
          );
      }
      // Empty query should return files (frecency-sorted)
      expect(result.value.totalFiles).toBeGreaterThan(0);
    } else {
    }
  });

  test("search returns a valid result structure", () => {
    const result = finder.fileSearch("Cargo.toml");
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(typeof result.value.totalMatched).toBe("number");
      expect(typeof result.value.totalFiles).toBe("number");
      expect(Array.isArray(result.value.items)).toBe(true);
      expect(Array.isArray(result.value.scores)).toBe(true);
    }
  });

  test("search returns empty for non-matching query", () => {
    const result = finder.fileSearch("xyznonexistentfilenamexyz123456");
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.totalMatched).toBe(0);
      expect(result.value.items.length).toBe(0);
    }
  });

  test("search respects pageSize option", () => {
    const result = finder.fileSearch("ts", { pageSize: 3 });
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.items.length).toBeLessThanOrEqual(3);
    }
  });

  test("grep plain text returns matching lines", () => {
    const result = finder.grep("fff-core", {
      mode: "plain",
    });
    expect(result.ok).toBe(true);

    if (result.ok) {
      if (result.value.items.length > 0) {
        // Log sample match to verify content on Windows
        const first = result.value.items[0];
        const _normalizedPath = normalizePath(first.relativePath);
      }

      expect(result.value.totalMatched).toBeGreaterThan(0);
      expect(result.value.items.length).toBeGreaterThan(0);

      const first = result.value.items[0];
      expect(typeof first.relativePath).toBe("string");
      // Normalize path for cross-platform validation
      const normalizedFirstPath = normalizePath(first.relativePath);
      expect(normalizedFirstPath).toBeTruthy();
      expect(typeof first.lineNumber).toBe("number");
      expect(first.lineNumber).toBeGreaterThan(0);
      expect(typeof first.lineContent).toBe("string");
      expect(first.lineContent.toLowerCase()).toContain("fff-core");
      expect(Array.isArray(first.matchRanges)).toBe(true);
      expect(first.matchRanges.length).toBeGreaterThan(0);

      expect(typeof result.value.totalFilesSearched).toBe("number");
      expect(typeof result.value.totalFiles).toBe("number");
      expect(typeof result.value.filteredFileCount).toBe("number");
    } else {
    }
  });

  test("grep fuzzy mode returns results with scores", () => {
    // Intentional typo: "depdnency" instead of "dependency" to exercise fuzzy matching
    const result = finder.grep("depdnency", {
      mode: "fuzzy",
    });
    expect(result.ok).toBe(true);

    if (result.ok) {
      expect(result.value.totalMatched).toBeGreaterThan(0);
      expect(result.value.items.length).toBeGreaterThan(0);

      const first = result.value.items[0];
      expect(typeof first.relativePath).toBe("string");
      // Normalize path for cross-platform validation
      const normalizedFirstPath = normalizePath(first.relativePath);
      expect(normalizedFirstPath).toBeTruthy();
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
      // Normalize basePath for cross-platform comparison
      const normalizedBasePath = normalizePath(result.value.filePicker.basePath || "");
      const normalizedTestDir = normalizePath(testDir);
      expect(normalizedBasePath).toBe(normalizedTestDir);
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
      const search1 = finder.fileSearch("Cargo");
      const search2 = finder2.fileSearch("Cargo");

      expect(search1.ok).toBe(true);
      expect(search2.ok).toBe(true);

      // Destroying one should not affect the other
      finder2.destroy();

      const search3 = finder.fileSearch("Cargo");
      expect(search3.ok).toBe(true);
    }
  });
});

describe("FileFinder - Directory Search", () => {
  let finder: FileFinder;
  const tmpDir = path.join(testDir, "__test_dirs__");
  const sep = path.sep;

  beforeAll(() => {
    fs.mkdirSync(path.join(tmpDir, "alpha", "nested"), { recursive: true });
    fs.mkdirSync(path.join(tmpDir, "beta"), { recursive: true });
    fs.writeFileSync(path.join(tmpDir, "alpha", "file.txt"), "x");
    fs.writeFileSync(path.join(tmpDir, "alpha", "nested", "deep.txt"), "x");
    fs.writeFileSync(path.join(tmpDir, "beta", "file.txt"), "x");

    const result = FileFinder.create({ basePath: testDir });
    expect(result.ok).toBe(true);
    if (result.ok) {
      finder = result.value;
    }
    finder.waitForScan(5000);
  });

  afterAll(() => {
    finder?.destroy();
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  test("known directories are returned with correct paths", () => {
    const result = finder.directorySearch("__test_dirs__");
    expect(result.ok).toBe(true);
    if (!result.ok) return;

    const paths = result.value.items.map((i) => i.relativePath);
    expect(paths).toContain(`__test_dirs__${sep}alpha${sep}`);
    expect(paths).toContain(`__test_dirs__${sep}beta${sep}`);
    expect(paths).toContain(`__test_dirs__${sep}alpha${sep}nested${sep}`);
  });

  test("nested directory uses native separators and correct dirName", () => {
    const result = finder.directorySearch("nested");
    expect(result.ok).toBe(true);
    if (!result.ok) return;

    const nested = result.value.items.find((i) => i.relativePath.includes("nested"));
    expect(nested).toBeDefined();
    expect(nested!.relativePath).toBe(`__test_dirs__${sep}alpha${sep}nested${sep}`);
    expect(nested!.dirName).toBe(`nested${sep}`);
  });
});

describe("FileFinder - Error Handling", () => {
  test("search fails on destroyed instance", () => {
    const createResult = FileFinder.create({ basePath: testDir });
    expect(createResult.ok).toBe(true);
    if (!createResult.ok) return;

    const f = createResult.value;
    f.destroy();

    const result = f.fileSearch("test");
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
    // Use a cross-platform invalid path
    const invalidPath =
      process.platform === "win32"
        ? "C:\\nonexistent\\path\\that\\does\\not\\exist"
        : "/nonexistent/path/that/does/not/exist";

    const result = FileFinder.create({
      basePath: invalidPath,
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
