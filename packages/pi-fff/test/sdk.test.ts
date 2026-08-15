import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, test } from "bun:test";
import { loadFirst, sdkCandidates } from "../src/sdk";

describe("sdkCandidates", () => {
  const original = process.env.FFF_SDK;

  beforeEach(() => {
    process.env.FFF_SDK = original;
  });

  test("defaults to bun candidates under a Bun runtime", () => {
    delete process.env.FFF_SDK;
    expect(sdkCandidates()).toEqual(["@ff-labs/fff-bun", "@ff-labs/fff-node"]);
  });

  test("FFF_SDK=node forces the node candidates", () => {
    process.env.FFF_SDK = "node";
    expect(sdkCandidates()).toEqual(["@ff-labs/fff-node", "@ff-labs/fff-bun"]);
  });

  test("FFF_SDK=bun forces the bun candidates", () => {
    process.env.FFF_SDK = "bun";
    expect(sdkCandidates()).toEqual(["@ff-labs/fff-bun", "@ff-labs/fff-node"]);
  });

  test("unknown FFF_SDK falls back to runtime detection", () => {
    process.env.FFF_SDK = "nope";
    // test runner is Bun, so runtime detection yields the bun candidates
    expect(sdkCandidates()[0]).toBe("@ff-labs/fff-bun");
  });
});

describe("literal SDK imports", () => {
  test("both SDK specifiers appear as literal dynamic imports for static graph scans", () => {
    const source = readFileSync(new URL("../src/sdk.ts", import.meta.url), "utf8");
    expect(source).toContain('import("@ff-labs/fff-bun")');
    expect(source).toContain('import("@ff-labs/fff-node")');
  });
});

describe("loadFirst", () => {
  test("prefers the first candidate when both load", async () => {
    const loaders = {
      a: () => Promise.resolve({ FileFinder: { create: () => "a" } }),
      b: () => Promise.resolve({ FileFinder: { create: () => "b" } }),
    };
    const mod = await loadFirst(["a", "b"], loaders);
    expect(mod.FileFinder.create()).toBe("a");
  });

  test("falls back to the second candidate when the first import fails", async () => {
    const loaders = {
      a: () => Promise.reject(new Error("cannot find a")),
      b: () => Promise.resolve({ FileFinder: { create: () => "b" } }),
    };
    const mod = await loadFirst(["a", "b"], loaders);
    expect(mod.FileFinder.create()).toBe("b");
  });

  test("throws the last error when every candidate fails", async () => {
    const loaders = {
      a: () => Promise.reject(new Error("first failure")),
      b: () => Promise.reject(new Error("second failure")),
    };
    await expect(loadFirst(["a", "b"], loaders)).rejects.toThrow("second failure");
  });
});
