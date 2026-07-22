import { describe, expect, mock, test } from "bun:test";

interface MockFinder {
  isDestroyed: boolean;
  basePath: string;
  waitForScan: (ms: number) => Promise<void>;
  destroy: () => void;
}

const created: MockFinder[] = [];
const createOptions: Record<string, unknown>[] = [];

function createMockFinder(basePath: string): MockFinder {
  const finder: MockFinder = {
    isDestroyed: false,
    basePath,
    waitForScan: async () => {},
    destroy: () => {
      finder.isDestroyed = true;
    },
  };
  created.push(finder);
  return finder;
}

const finderModule = {
  FileFinder: {
    create: (options: Record<string, unknown>) => {
      createOptions.push(options);
      return {
        ok: true,
        value: createMockFinder(options.basePath as string),
      };
    },
  },
};

mock.module("@ff-labs/fff-node", () => finderModule);
mock.module("@ff-labs/fff-bun", () => finderModule);

const { AuxFinderPool } = await import("../src/aux-finders");

function makePool() {
  created.length = 0;
  createOptions.length = 0;
  return new AuxFinderPool({ enableFsRootScanning: false });
}

describe("AuxFinderPool covering reuse", () => {
  test("reuses a picker rooted at an ancestor of the requested path", async () => {
    const pool = makePool();
    const a = await pool.acquire("/a/b/c");
    expect(a.root).toBe("/a/b/c");

    const b = await pool.acquire("/a/b/c/d");
    expect(b.finder).toBe(a.finder);
    expect(b.root).toBe("/a/b/c");
    expect(created.length).toBe(1);
  });

  test("does not reuse a picker rooted deeper than the requested path", async () => {
    const pool = makePool();
    await pool.acquire("/a/b/c");
    const broad = await pool.acquire("/a/b");
    expect(broad.root).toBe("/a/b");
    expect(created.length).toBe(2);
  });

  test("prefers the deepest covering picker", async () => {
    const pool = makePool();
    const narrow = await pool.acquire("/a/b/c");
    await pool.acquire("/a/b");

    const again = await pool.acquire("/a/b/c/src");
    expect(again.finder).toBe(narrow.finder);
    expect(again.root).toBe("/a/b/c");
  });

  test("exact mode skips ancestor reuse", async () => {
    const pool = makePool();
    await pool.acquire("/a/b");
    const exact = await pool.acquire("/a/b/c", { exact: true });
    expect(exact.root).toBe("/a/b/c");
    expect(created.length).toBe(2);
  });

  test("does not treat sibling prefixes as covering", async () => {
    const pool = makePool();
    await pool.acquire("/a/bc");
    const other = await pool.acquire("/a/b");
    expect(other.root).toBe("/a/b");
    expect(created.length).toBe(2);
  });

  // Regression for #700: aux finders must not reopen the main frecency/history
  // LMDB envs, or heed fails with "environment already open in this program".
  test("aux finders are created without frecency/history db paths", async () => {
    const pool = makePool();
    await pool.acquire("/a/b/c");
    await pool.acquire("/x/y");
    expect(createOptions.length).toBe(2);
    for (const opts of createOptions) {
      expect(opts.frecencyDbPath).toBeUndefined();
      expect(opts.historyDbPath).toBeUndefined();
    }
  });
});
