import { describe, expect, mock, test } from "bun:test";

const waitForScan = mock(async () => undefined);
const finder = { waitForScan };
const finderModule = {
  FileFinder: {
    create: mock(() => ({ ok: true, value: finder })),
  },
};

mock.module("@ff-labs/fff-node", () => finderModule);
mock.module("@ff-labs/fff-bun", () => finderModule);

const { FilePickerFactory } = await import("../src/file-picker");

describe("FilePickerFactory", () => {
  test("returns the picker without waiting for its initial scan", async () => {
    const factory = new FilePickerFactory({
      frecencyDbPath: "/dbs/frecency",
      historyDbPath: "/dbs/history",
    });

    const result = await factory.create({ basePath: "/workspace" });

    expect(result).toBeDefined();
    expect(waitForScan).not.toHaveBeenCalled();
  });
});
