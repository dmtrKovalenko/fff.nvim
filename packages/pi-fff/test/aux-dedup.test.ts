import { describe, expect, mock, test } from "bun:test";

interface MockFinder {
  isDestroyed: boolean;
  basePath: string;
  waitForScan: (ms: number) => Promise<void>;
  destroy: () => void;
}

const created: MockFinder[] = [];

function createMockFinder(basePath: string): MockFinder {
  const finder: MockFinder = {
    isDestroyed: false,
    basePath,
    // simulate a slow scan so concurrent acquires overlap
    waitForScan: () => new Promise((r) => setTimeout(r, 50)),
    destroy: () => {
      finder.isDestroyed = true;
    },
  };
  created.push(finder);
  return finder;
}

const finderModule = {
  FileFinder: {
    create: (options: Record<string, unknown>) => ({
      ok: true,
      value: createMockFinder(options.basePath as string),
    }),
  },
};

mock.module("@ff-labs/fff-node", () => finderModule);
mock.module("@ff-labs/fff-bun", () => finderModule);

const { AuxFinderPool } = await import("../src/aux-finders");

describe("AuxFinderPool concurrent dedup (#746)", () => {
  test("two concurrent acquires for same root share one finder", async () => {
    created.length = 0;
    const pool = new AuxFinderPool({ enableFsRootScanning: false });
    const [a, b] = await Promise.all([
      pool.acquire("/Users/x"),
      pool.acquire("/Users/x"),
    ]);
    expect(created.length).toBe(1);
    expect(a.finder).toBe(b.finder);
  });

  test("sequential acquire after in-flight one resolves still reuses", async () => {
    created.length = 0;
    const pool = new AuxFinderPool({ enableFsRootScanning: false });
    const first = pool.acquire("/Users/x");
    const second = pool.acquire("/Users/x");
    await Promise.all([first, second]);
    const third = await pool.acquire("/Users/x");
    expect(created.length).toBe(1);
    expect(third.root).toBe("/Users/x");
  });
});
