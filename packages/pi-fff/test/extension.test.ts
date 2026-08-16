import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

type MockFinder = {
  isDestroyed: boolean;
  waitForScan: ReturnType<typeof mock>;
  mixedSearch: ReturnType<typeof mock>;
  getScanProgress: ReturnType<typeof mock>;
  destroy: ReturnType<typeof mock>;
};

const createCalls: unknown[] = [];
let finders: MockFinder[] = [];
let mixedSearchImpl: ((query: string, options: unknown) => unknown) | undefined;
let scanProgressImpl: (() => unknown) | undefined;

function createMockFinder(): MockFinder {
  return {
    isDestroyed: false,
    waitForScan: mock(async () => undefined),
    getScanProgress: mock(() => {
      if (scanProgressImpl) return scanProgressImpl();
      return {
        ok: true,
        value: {
          scannedFilesCount: 0,
          isScanning: false,
          isWatcherReady: true,
          isWarmupComplete: true,
        },
      };
    }),
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

const finderModule = {
  FileFinder: {
    create: mock((options: unknown) => {
      createCalls.push(options);
      const finder = createMockFinder();
      finders.push(finder);
      return { ok: true, value: finder };
    }),
  },
};

mock.module("@ff-labs/fff-node", () => finderModule);
mock.module("@ff-labs/fff-bun", () => finderModule);

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
    Optional: (value: Record<string, unknown>) => ({ ...value, optional: true }),
    String: schema("string"),
    Union: (items: unknown[], options?: unknown) => ({ type: "union", items, options }),
  },
}));

const { default: fffExtension } = await import("../src/index");

type EventHandler = (...args: any[]) => unknown;

function createPi(mode?: string, flags: Record<string, unknown> = {}) {
  const events = new Map<string, EventHandler>();
  const commands = new Map<string, any>();

  const pi = {
    getFlag: mock((name: string) =>
      name === "fff-mode" && mode !== undefined ? mode : flags[name],
    ),
    on: mock((event: string, handler: EventHandler) => {
      events.set(event, handler);
    }),
    registerCommand: mock((name: string, command: any) => {
      commands.set(name, command);
    }),
    registerFlag: mock(() => undefined),
    registerTool: mock((_tool: any) => undefined),
    appendEntry: mock(() => undefined),
  };

  return { pi, events, commands };
}

function createContext(cwd = "/tmp/workspace") {
  return {
    cwd,
    // Signatures mirror the real pi UI surface so mock.calls stays typed.
    ui: {
      addAutocompleteProvider: mock((_factory: (current: any) => any) => undefined),
      notify: mock((_message: string, _level?: string) => undefined),
      setEditorComponent: mock(() => undefined),
      setStatus: mock((_key: string, _text?: string) => undefined),
    },
  };
}

async function start(mode?: string, cwd?: string, flags: Record<string, unknown> = {}) {
  const setup = createPi(mode, flags);
  const ctx = createContext(cwd);
  fffExtension(setup.pi as any);

  const sessionStart = setup.events.get("session_start");
  expect(sessionStart).toBeDefined();
  await sessionStart?.({ reason: "startup" }, ctx);

  return { ...setup, ctx };
}

async function shutdown(setup: { events: Map<string, EventHandler> }) {
  await setup.events.get("session_shutdown")?.({}, undefined);
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

const CONFIG_ENV_KEYS = [
  "PI_CODING_AGENT_DIR",
  "PI_FFF_MODE",
  "FFF_FRECENCY_DB",
  "FFF_HISTORY_DB",
  "FFF_ENABLE_ROOT_SCAN",
  "FFF_ENABLE_HOME_SCAN",
] as const;

const savedEnv: Record<string, string | undefined> = {};
for (const key of CONFIG_ENV_KEYS) savedEnv[key] = process.env[key];

const agentDir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-fff-extension-"));
const configPath = path.join(agentDir, "pi-fff.json");

beforeEach(() => {
  createCalls.length = 0;
  finders = [];
  mixedSearchImpl = undefined;
  scanProgressImpl = undefined;

  for (const key of CONFIG_ENV_KEYS) delete process.env[key];
  process.env.PI_CODING_AGENT_DIR = agentDir;
  fs.rmSync(configPath, { force: true });
});

afterAll(() => {
  for (const key of CONFIG_ENV_KEYS) {
    const value = savedEnv[key];
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
  fs.rmSync(agentDir, { recursive: true, force: true });
});

describe("pi-fff global config", () => {
  test("applies every supported startup option", async () => {
    writeConfig({
      mode: "override",
      frecencyDbPath: "/config/frecency",
      historyDbPath: "/config/history",
      enableFsRootScanning: true,
      enableHomeDirScanning: false,
    });

    const setup = await start();
    const toolNames = setup.pi.registerTool.mock.calls.map(([tool]) => tool.name);

    expect(toolNames).toContain("grep");
    expect(toolNames).toContain("find");
    expect(toolNames).not.toContain("ffgrep");
    expect(createCalls[0]).toEqual({
      basePath: "/tmp/workspace",
      frecencyDbPath: "/config/frecency",
      historyDbPath: "/config/history",
      aiMode: true,
      enableHomeDirScanning: false,
      enableFsRootScanning: true,
    });
    await shutdown(setup);
  });

  test("keeps flag and environment precedence", async () => {
    writeConfig({
      mode: "tools-only",
      frecencyDbPath: "/config/frecency",
      historyDbPath: "/config/history",
      enableFsRootScanning: true,
      enableHomeDirScanning: false,
    });
    process.env.PI_FFF_MODE = "override";
    process.env.FFF_FRECENCY_DB = "/env/frecency";
    process.env.FFF_HISTORY_DB = "/env/history";
    process.env.FFF_ENABLE_ROOT_SCAN = "1";
    process.env.FFF_ENABLE_HOME_SCAN = "1";

    const setup = await start("tools-and-ui", undefined, {
      "fff-frecency-db": "/flag/frecency",
      "fff-enable-root-scan": false,
    });
    const toolNames = setup.pi.registerTool.mock.calls.map(([tool]) => tool.name);

    expect(toolNames).toContain("ffgrep");
    expect(toolNames).toContain("fffind");
    expect(createCalls[0]).toEqual({
      basePath: "/tmp/workspace",
      frecencyDbPath: "/flag/frecency",
      historyDbPath: "/env/history",
      aiMode: true,
      enableHomeDirScanning: true,
      enableFsRootScanning: false,
    });
    await shutdown(setup);
  });
});

function writeConfig(config: Record<string, unknown>): void {
  fs.writeFileSync(configPath, JSON.stringify(config));
}

// Regression for #743: launching from $HOME must be visible and interruptible.
describe("pi-fff $HOME scan warning", () => {
  test("warns and pins a status when cwd is $HOME", async () => {
    const setup = await start(undefined, os.homedir());

    expect(setup.ctx.ui.notify).toHaveBeenCalledTimes(1);
    const [message, level] = setup.ctx.ui.notify.mock.calls[0];
    expect(message).toContain(os.homedir());
    expect(level).toBe("warning");
    expect(setup.ctx.ui.setStatus).toHaveBeenCalledWith(
      "fff",
      "Agent is indexing $HOME, this can lead to high CPU",
    );
    await shutdown(setup);
  });

  test("stays silent outside $HOME", async () => {
    const { ctx } = await start();

    expect(ctx.ui.notify).not.toHaveBeenCalled();
    expect(ctx.ui.setStatus).not.toHaveBeenCalled();
  });

  test("clears the status once the scan settles", async () => {
    const setup = await start(undefined, os.homedir());

    expect(setup.ctx.ui.setStatus).toHaveBeenLastCalledWith("fff", undefined);
    await shutdown(setup);
  });

  // waitForScan resolves on timeout, so a slow $HOME walk keeps the footer up.
  test("keeps reporting live progress while the scan is still running", async () => {
    scanProgressImpl = () => ({
      ok: true,
      value: {
        scannedFilesCount: 12345,
        isScanning: true,
        isWatcherReady: false,
        isWarmupComplete: false,
      },
    });
    const setup = await start(undefined, os.homedir());

    const lastStatus = setup.ctx.ui.setStatus.mock.calls.at(-1);
    expect(lastStatus?.[0]).toBe("fff");
    expect(lastStatus?.[1]).toContain("12345 files");

    // session_shutdown must stop the poller and clear the footer.
    await shutdown(setup);
    expect(setup.ctx.ui.setStatus).toHaveBeenLastCalledWith("fff", undefined);
  });

  test("no warning when home scanning is disabled", async () => {
    process.env.FFF_ENABLE_HOME_SCAN = "0";
    const setup = await start(undefined, os.homedir());

    expect(setup.ctx.ui.notify).not.toHaveBeenCalled();
    expect(setup.ctx.ui.setStatus).not.toHaveBeenCalled();
    await shutdown(setup);
  });
});

describe("pi-fff autocomplete registration", () => {
  test("session_start registers a provider without replacing the editor", async () => {
    const { ctx } = await start();

    expect(ctx.ui.addAutocompleteProvider).toHaveBeenCalledTimes(1);
    expect(ctx.ui.setEditorComponent).not.toHaveBeenCalled();
    expect(createCalls).toEqual([
      {
        basePath: "/tmp/workspace",
        // Resolved defaults are host-dependent; covered by test/db-paths.test.ts.
        frecencyDbPath: expect.any(String),
        historyDbPath: expect.any(String),
        aiMode: true,
        enableHomeDirScanning: true,
        enableFsRootScanning: false,
      },
    ]);
  });

  test("FFF_ENABLE_HOME_SCAN=0 disables home dir scanning", async () => {
    process.env.FFF_ENABLE_HOME_SCAN = "0";
    await start();

    const opts = createCalls[0] as { enableHomeDirScanning: boolean };
    expect(opts.enableHomeDirScanning).toBe(false);
  });

  test("session_start survives hosts without addAutocompleteProvider", async () => {
    const setup = createPi();
    const ctx = {
      cwd: "/tmp/workspace",
      ui: {
        notify: mock(() => undefined),
        setEditorComponent: mock(() => undefined),
      },
    };
    fffExtension(setup.pi as any);

    const sessionStart = setup.events.get("session_start");
    await sessionStart?.({ reason: "startup" }, ctx);

    expect(ctx.ui.notify).not.toHaveBeenCalled();
    expect(createCalls).toHaveLength(1);
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
