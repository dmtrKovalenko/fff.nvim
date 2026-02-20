import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import { FileFinder } from "./index";
import type { FileItem } from "./types";
import {
  mkdtempSync,
  writeFileSync,
  rmSync,
  unlinkSync,
  mkdirSync,
  realpathSync,
} from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { execSync } from "node:child_process";

/**
 * Integration test: full git lifecycle with a real repository.
 *
 * Creates a temporary git repo, initialises a FileFinder instance pointing at
 * it, then walks through:
 *   1. Initial scan – committed files should have status "clean"
 *   2. Add a new untracked file – should appear as "untracked"
 *   3. Stage the new file – should appear as "staged_new"
 *   4. Commit – should become "clean"
 *   5. Modify a tracked file – should become "modified"
 *   6. Stage the modification – should become "staged_modified"
 *   7. Commit again – back to "clean"
 *   8. Delete a file – should disappear from the index
 */

const POLL_INTERVAL_MS = 100;
const WATCHER_TIMEOUT_MS = 5_000; // generous CI timeout; polls exit early on fast machines

function git(cwd: string, ...args: string[]) {
  const escaped = args.map((a) => `'${a.replace(/'/g, "'\\''")}'`).join(" ");
  execSync(`git ${escaped}`, {
    cwd,
    stdio: "pipe",
    env: {
      ...process.env,
      GIT_AUTHOR_NAME: "test",
      GIT_AUTHOR_EMAIL: "test@test.com",
      GIT_COMMITTER_NAME: "test",
      GIT_COMMITTER_EMAIL: "test@test.com",
    },
  });
}

function sleep(ms: number) {
  return new Promise((r) => setTimeout(r, ms));
}

function findFile(finder: FileFinder, name: string): FileItem | undefined {
  const result = finder.search(name, { pageSize: 200 });
  if (!result.ok) throw new Error(`search failed: ${result.error}`);
  return result.value.items.find((item) => item.fileName === name);
}

/** Poll until a file appears in the index, or the timeout is exceeded. */
async function waitForFile(
  finder: FileFinder,
  name: string,
): Promise<FileItem | undefined> {
  const start = Date.now();
  while (Date.now() - start < WATCHER_TIMEOUT_MS) {
    const file = findFile(finder, name);
    if (file !== undefined) return file;
    await sleep(POLL_INTERVAL_MS);
  }
  return findFile(finder, name);
}

/** Poll until a file has the expected git status, or the timeout is exceeded. */
async function waitForFileStatus(
  finder: FileFinder,
  name: string,
  status: string,
): Promise<FileItem | undefined> {
  const start = Date.now();
  while (Date.now() - start < WATCHER_TIMEOUT_MS) {
    const file = findFile(finder, name);
    if (file?.gitStatus === status) return file;
    await sleep(POLL_INTERVAL_MS);
  }
  return findFile(finder, name);
}

/** Poll until a file is gone from the index, or the timeout is exceeded. */
async function waitForFileGone(
  finder: FileFinder,
  name: string,
): Promise<boolean> {
  const start = Date.now();
  while (Date.now() - start < WATCHER_TIMEOUT_MS) {
    if (findFile(finder, name) === undefined) return true;
    await sleep(POLL_INTERVAL_MS);
  }
  return findFile(finder, name) === undefined;
}

/** Poll until the total file count reaches the expected value, or the timeout is exceeded. */
async function waitForFileCount(
  finder: FileFinder,
  count: number,
): Promise<number> {
  const start = Date.now();
  while (Date.now() - start < WATCHER_TIMEOUT_MS) {
    const result = finder.search("", { pageSize: 200 });
    if (result.ok && result.value.totalFiles === count) return count;
    await sleep(POLL_INTERVAL_MS);
  }
  const result = finder.search("", { pageSize: 200 });
  return result.ok ? result.value.totalFiles : -1;
}

/** Poll liveGrep until predicate on totalMatched is satisfied, or the timeout is exceeded. */
async function waitForGrep(
  finder: FileFinder,
  pattern: string,
  options: { mode: "plain" | "regex" },
  predicate: (totalMatched: number) => boolean,
) {
  const start = Date.now();
  while (Date.now() - start < WATCHER_TIMEOUT_MS) {
    const result = finder.liveGrep(pattern, options);
    if (result.ok && predicate(result.value.totalMatched)) return result;
    await sleep(POLL_INTERVAL_MS);
  }
  return finder.liveGrep(pattern, options);
}

describe.skipIf(process.platform === "win32")(
  "Git lifecycle integration",
  () => {
    let tmpDir: string;
    let finder: FileFinder;

    beforeAll(() => {
      // Create temp directory and initialise a git repo with two committed files.
      // Use realpathSync to resolve symlinks (macOS /var -> /private/var) so
      // that git2's resolved workdir paths match the file picker's base_path.
      tmpDir = realpathSync(mkdtempSync(join(tmpdir(), "fff-git-test-")));

      git(tmpDir, "init", "-b", "main");
      // Need at least one commit for status to work properly
      writeFileSync(join(tmpDir, "hello.txt"), "hello world\n");
      writeFileSync(join(tmpDir, "readme.md"), "# Test Project\n");
      mkdirSync(join(tmpDir, "src"));
      writeFileSync(
        join(tmpDir, "src", "main.rs"),
        'fn main() { println?."hi"); }\n',
      );
      git(tmpDir, "add", "-A");
      git(tmpDir, "commit", "-m", "initial commit");

      // Create the FileFinder instance
      const result = FileFinder.create({ basePath: tmpDir });
      expect(result.ok).toBe(true);
      if (!result.ok) throw new Error(result.error);
      finder = result.value;

      // Wait for the initial scan to finish
      const scanResult = finder.waitForScan(10_000);
      expect(scanResult.ok).toBe(true);
    });

    afterAll(() => {
      finder?.destroy();
      if (tmpDir) {
        rmSync(tmpDir, { recursive: true, force: true });
      }
    });

    test("initial scan indexes all committed files", () => {
      const result = finder.search("", { pageSize: 200 });
      expect(result.ok).toBe(true);
      if (!result.ok) return;

      const names = result.value.items.map((i) => i.relativePath).sort();
      expect(names).toContain("hello.txt");
      expect(names).toContain("readme.md");
      expect(names).toContain("src/main.rs");
      expect(result.value.totalFiles).toBe(3);
    });

    test("committed files have clean git status", async () => {
      const hello = await waitForFileStatus(finder, "hello.txt", "clean");
      expect(hello).toBeDefined();
      expect(hello?.gitStatus).toBe("clean");

      const main = await waitForFileStatus(finder, "main.rs", "clean");
      expect(main).toBeDefined();
      expect(main?.gitStatus).toBe("clean");
    });

    test("new untracked file appears with 'untracked' status", async () => {
      writeFileSync(join(tmpDir, "new_file.ts"), "export const x = 1;\n");

      const newFile = await waitForFileStatus(finder, "new_file.ts", "untracked");
      expect(newFile).toBeDefined();
      expect(newFile?.gitStatus).toBe("untracked");

      // Total should now be 4
      const total = await waitForFileCount(finder, 4);
      expect(total).toBe(4);
    });

    test("staging a new file changes status to 'staged_new'", async () => {
      git(tmpDir, "add", "new_file.ts");

      const newFile = await waitForFileStatus(finder, "new_file.ts", "staged_new");
      expect(newFile).toBeDefined();
      expect(newFile?.gitStatus).toBe("staged_new");
    });

    test("committing makes the file 'clean'", async () => {
      git(tmpDir, "commit", "-m", "add new_file");

      const newFile = await waitForFileStatus(finder, "new_file.ts", "clean");
      expect(newFile).toBeDefined();
      expect(newFile?.gitStatus).toBe("clean");
    });

    test("modifying a tracked file changes status to 'modified'", async () => {
      writeFileSync(
        join(tmpDir, "hello.txt"),
        "hello world\nupdated content\n",
      );

      const hello = await waitForFileStatus(finder, "hello.txt", "modified");
      expect(hello).toBeDefined();
      expect(hello?.gitStatus).toBe("modified");
    });

    test("staging a modification changes status to 'staged_modified'", async () => {
      git(tmpDir, "add", "hello.txt");

      const hello = await waitForFileStatus(finder, "hello.txt", "staged_modified");
      expect(hello).toBeDefined();
      expect(hello?.gitStatus).toBe("staged_modified");
    });

    test("committing the modification returns to 'clean'", async () => {
      git(tmpDir, "commit", "-m", "update hello");

      const hello = await waitForFileStatus(finder, "hello.txt", "clean");
      expect(hello).toBeDefined();
      expect(hello?.gitStatus).toBe("clean");
    });

    test("deleting a file removes it from the index", async () => {
      unlinkSync(join(tmpDir, "new_file.ts"));

      const gone = await waitForFileGone(finder, "new_file.ts");
      expect(gone).toBe(true);

      // Total should be back to 3
      const total = await waitForFileCount(finder, 3);
      expect(total).toBe(3);
    });

    test("adding a file in a subdirectory works", async () => {
      writeFileSync(join(tmpDir, "src", "utils.rs"), "pub fn helper() {}\n");

      const utils = await waitForFileStatus(finder, "utils.rs", "untracked");
      expect(utils).toBeDefined();
      expect(utils?.relativePath).toBe("src/utils.rs");
      expect(utils?.gitStatus).toBe("untracked");
    });

    test("live grep finds content in a newly added file", async () => {
      writeFileSync(
        join(tmpDir, "src", "searchtarget.rs"),
        'const UNIQUE_NEEDLE: &str = "xylophone_waterfall_97";\n',
      );

      await waitForFile(finder, "searchtarget.rs");

      const result = await waitForGrep(
        finder,
        "xylophone_waterfall_97",
        { mode: "plain" },
        (n) => n > 0,
      );
      expect(result?.ok).toBe(true);
      if (!result?.ok) return;

      expect(result.value.totalMatched).toBeGreaterThan(0);
      const match = result.value.items.find(
        (m) => m.relativePath === "src/searchtarget.rs",
      );
      expect(match).toBeDefined();
      expect(match!.lineContent).toContain("xylophone_waterfall_97");
    });

    test("live grep no longer finds content after file is deleted", async () => {
      unlinkSync(join(tmpDir, "src", "searchtarget.rs"));

      const result = await waitForGrep(
        finder,
        "xylophone_waterfall_97",
        { mode: "plain" },
        (n) => n === 0,
      );
      expect(result?.ok).toBe(true);
      if (!result?.ok) return;

      expect(result.value.totalMatched).toBe(0);
      expect(result.value.items.length).toBe(0);
    });

    test("full add-commit cycle for subdirectory file", async () => {
      git(tmpDir, "add", "src/utils.rs");

      let utils = await waitForFileStatus(finder, "utils.rs", "staged_new");
      expect(utils).toBeDefined();
      expect(utils?.gitStatus).toBe("staged_new");

      git(tmpDir, "commit", "-m", "add utils");

      utils = await waitForFileStatus(finder, "utils.rs", "clean");
      expect(utils).toBeDefined();
      expect(utils?.gitStatus).toBe("clean");
    });
  },
);
