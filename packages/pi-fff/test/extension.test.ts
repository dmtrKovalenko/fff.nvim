import { beforeEach, describe, expect, mock, test } from "bun:test";

type MockFinder = {
  isDestroyed: boolean;
  waitForScan: ReturnType<typeof mock>;
  mixedSearch: ReturnType<typeof mock>;
  destroy: ReturnType<typeof mock>;
};

const createCalls: unknown[] = [];
let finders: MockFinder[] = [];
let mixedSearchImpl: ((query: string, options: unknown) => unknown) | undefined;

function createMockFinder(): MockFinder {
  return {
    isDestroyed: false,
    waitForScan: mock(async () => undefined),
    mixedSearch: mock((query: string, options: unknown) => {
      if (mixedSearchImpl) return mixedSearchImpl(query, options);
      return {
        ok: true,
        value: {
          items: [],
          scores: [],
          totalMatched: 0,
          totalFiles: 0,
          totalDirs: 0,
        },
      };
    }),
    destroy: mock(function (this: MockFinder) {
      this.isDestroyed = true;
    }),
  };
}

mock.module("@ff-labs/fff-node", () => ({
  FileFinder: {
    create: mock((options: unknown) => {
      createCalls.push(options);
      const finder = createMockFinder();
      finders.push(finder);
      return { ok: true, value: finder };
    }),
  },
}));

mock.module("@earendil-works/pi-tui", () => ({
  Text: class Text {
    text: string;
    constructor(text: string) {
      this.text = text;
    }
    setText(text: string) {
      this.text = text;
    }
  },
}));

const schema = (type: string) => (options?: unknown) => ({ type, options });

mock.module("@sinclair/typebox", () => ({
  Type: {
    Array: (items: unknown, options?: unknown) => ({ type: "array", items, options }),
    Boolean: schema("boolean"),
    Number: schema("number"),
    Object: (properties: unknown, options?: unknown) => ({
      type: "object",
      properties,
      options,
    }),
    Optional: (value: unknown) => ({ ...value, optional: true }),
    String: schema("string"),
    Union: (items: unknown[], options?: unknown) => ({ type: "union", items, options }),
  },
}));

const { default: fffExtension } = await import("../src/index");

type EventHandler = (...args: any[]) => unknown;

function createPi(mode?: string) {
  const events = new Map<string, EventHandler>();
  const commands = new Map<string, any>();

  const pi = {
    getFlag: mock((name: string) => (name === "fff-mode" ? mode : undefined)),
    on: mock((event: string, handler: EventHandler) => {
      events.set(event, handler);
    }),
    registerCommand: mock((name: string, command: any) => {
      commands.set(name, command);
    }),
    registerFlag: mock(() => undefined),
    registerTool: mock(() => undefined),
    appendEntry: mock(() => undefined),
  };

  return { pi, events, commands };
}

function createContext() {
  return {
    cwd: "/tmp/workspace",
    ui: {
      addAutocompleteProvider: mock(() => undefined),
      notify: mock(() => undefined),
      setEditorComponent: mock(() => undefined),
    },
  };
}

async function start(mode?: string) {
  const setup = createPi(mode);
  const ctx = createContext();
  fffExtension(setup.pi as any);

  const sessionStart = setup.events.get("session_start");
  expect(sessionStart).toBeDefined();
  await sessionStart?.({ reason: "startup" }, ctx);

  return { ...setup, ctx };
}

function currentProvider(
  result = { items: [{ value: "base", label: "base" }], prefix: "ba" },
) {
  return {
    getSuggestions: mock(async () => result),
    applyCompletion: mock(() => ({ lines: ["applied"], cursorLine: 0, cursorCol: 7 })),
    shouldTriggerFileCompletion: mock(() => false),
  };
}

function abortOptions() {
  return { signal: new AbortController().signal };
}

beforeEach(() => {
  createCalls.length = 0;
  finders = [];
  mixedSearchImpl = undefined;
  delete process.env.PI_FFF_MODE;
});

describe("pi-fff autocomplete registration", () => {
  test("session_start registers a provider without replacing the editor", async () => {
    const { ctx } = await start();

    expect(ctx.ui.addAutocompleteProvider).toHaveBeenCalledTimes(1);
    expect(ctx.ui.setEditorComponent).not.toHaveBeenCalled();
    expect(createCalls).toEqual([
      {
        basePath: "/tmp/workspace",
        frecencyDbPath: undefined,
        historyDbPath: undefined,
        aiMode: true,
        enableHomeDirScanning: true,
        enableFsRootScanning: false,
      },
    ]);
  });

  test("delegates non-@ completions to the current provider", async () => {
    const { ctx } = await start();
    const factory = ctx.ui.addAutocompleteProvider.mock.calls[0][0];
    const current = currentProvider();
    const provider = factory(current);

    const result = await provider.getSuggestions(["hello"], 0, 5, abortOptions());

    expect(result).toEqual({ items: [{ value: "base", label: "base" }], prefix: "ba" });
    expect(current.getSuggestions).toHaveBeenCalledTimes(1);
    expect(finders[0].mixedSearch).not.toHaveBeenCalled();
  });

  test("returns FFF-backed @ mention suggestions", async () => {
    mixedSearchImpl = (query, options) => {
      expect(query).toBe("src");
      expect(options).toEqual({ pageSize: 20 });
      return {
        ok: true,
        value: {
          items: [
            {
              type: "file",
              item: {
                relativePath: "src/index.ts",
                fileName: "index.ts",
                size: 1,
                modified: 1,
                accessFrecencyScore: 0,
                modificationFrecencyScore: 0,
                totalFrecencyScore: 0,
                gitStatus: "clean",
              },
            },
            {
              type: "directory",
              item: {
                relativePath: "src/components/",
                dirName: "components/",
                maxAccessFrecency: 0,
              },
            },
          ],
          scores: [],
          totalMatched: 2,
          totalFiles: 1,
          totalDirs: 1,
        },
      };
    };

    const { ctx } = await start();
    const factory = ctx.ui.addAutocompleteProvider.mock.calls[0][0];
    const current = currentProvider();
    const provider = factory(current);

    const result = await provider.getSuggestions(["open @src"], 0, 9, abortOptions());

    expect(result).toEqual({
      prefix: "@src",
      items: [
        {
          value: "@src/index.ts",
          label: "index.ts",
          description: "src/index.ts",
        },
        {
          value: "@src/components/",
          label: "components/",
          description: "src/components/",
        },
      ],
    });
    expect(current.getSuggestions).not.toHaveBeenCalled();
  });

  test("delegates when FFF lookup fails", async () => {
    mixedSearchImpl = () => {
      throw new Error("native lookup failed");
    };

    const { ctx } = await start();
    const factory = ctx.ui.addAutocompleteProvider.mock.calls[0][0];
    const current = currentProvider();
    const provider = factory(current);

    const result = await provider.getSuggestions(["@src"], 0, 4, abortOptions());

    expect(result).toEqual({ items: [{ value: "base", label: "base" }], prefix: "ba" });
    expect(current.getSuggestions).toHaveBeenCalledTimes(1);
  });

  test("tools-only mode bypasses FFF mentions and delegates", async () => {
    const { ctx } = await start("tools-only");
    const factory = ctx.ui.addAutocompleteProvider.mock.calls[0][0];
    const current = currentProvider();
    const provider = factory(current);

    const result = await provider.getSuggestions(["@src"], 0, 4, abortOptions());

    expect(result).toEqual({ items: [{ value: "base", label: "base" }], prefix: "ba" });
    expect(current.getSuggestions).toHaveBeenCalledTimes(1);
    expect(finders[0].mixedSearch).not.toHaveBeenCalled();
  });

  test("/fff-mode changes mention behavior without touching the editor", async () => {
    const { commands, ctx } = await start();
    const factory = ctx.ui.addAutocompleteProvider.mock.calls[0][0];
    const current = currentProvider();
    const provider = factory(current);

    await commands.get("fff-mode").handler("tools-only", ctx);
    await provider.getSuggestions(["@src"], 0, 4, abortOptions());

    expect(current.getSuggestions).toHaveBeenCalledTimes(1);
    expect(finders[0].mixedSearch).not.toHaveBeenCalled();
    expect(ctx.ui.setEditorComponent).not.toHaveBeenCalled();
  });

  test("completion application and file-completion trigger delegate to current provider", async () => {
    const { ctx } = await start();
    const factory = ctx.ui.addAutocompleteProvider.mock.calls[0][0];
    const current = currentProvider();
    const provider = factory(current);

    const applied = provider.applyCompletion(
      ["@src"],
      0,
      4,
      { value: "@src/index.ts", label: "index.ts" },
      "@src",
    );
    const shouldTrigger = provider.shouldTriggerFileCompletion(["@src"], 0, 4);

    expect(applied).toEqual({ lines: ["applied"], cursorLine: 0, cursorCol: 7 });
    expect(shouldTrigger).toBe(false);
    expect(current.applyCompletion).toHaveBeenCalledTimes(1);
    expect(current.shouldTriggerFileCompletion).toHaveBeenCalledTimes(1);
  });
});
