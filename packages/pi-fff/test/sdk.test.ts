import { beforeEach, describe, expect, test } from "bun:test";
import { sdkCandidates } from "../src/sdk";

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
