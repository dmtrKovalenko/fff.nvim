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

const WATCHER_SETTLE_MS = 500; // accompany for the debouncer and replicate real life uasage

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
      // Wait for background watcher to process initial git status
      await sleep(WATCHER_SETTLE_MS);

      const hello = findFile(finder, "hello.txt");
      expect(hello).toBeDefined();
      expect(hello?.gitStatus).toBe("clean");

      const main = findFile(finder, "main.rs");
      expect(main).toBeDefined();
      expect(main?.gitStatus).toBe("clean");
    });

    test("new untracked file appears with 'untracked' status", async () => {
      writeFileSync(join(tmpDir, "new_file.ts"), "export const x = 1;\n");

      // Wait for the background watcher to pick up the change and update git status
      await sleep(WATCHER_SETTLE_MS);

      const newFile = findFile(finder, "new_file.ts");
      expect(newFile).toBeDefined();
      expect(newFile?.gitStatus).toBe("untracked");

      // Total should now be 4
      const all = finder.search("", { pageSize: 200 });
      expect(all.ok).toBe(true);
      if (all.ok) {
        expect(all.value.totalFiles).toBe(4);
      }
    });

    test("staging a new file changes status to 'staged_new'", async () => {
      git(tmpDir, "add", "new_file.ts");

      // Wait for background watcher to detect .git/index change
      await sleep(WATCHER_SETTLE_MS);

      const newFile = findFile(finder, "new_file.ts");
      expect(newFile).toBeDefined();
      expect(newFile?.gitStatus).toBe("staged_new");
    });

    test("committing makes the file 'clean'", async () => {
      git(tmpDir, "commit", "-m", "add new_file");

      // Wait for background watcher to detect .git changes
      await sleep(WATCHER_SETTLE_MS);

      const newFile = findFile(finder, "new_file.ts");
      expect(newFile).toBeDefined();
      expect(newFile?.gitStatus).toBe("clean");
    });

    test("modifying a tracked file changes status to 'modified'", async () => {
      writeFileSync(
        join(tmpDir, "hello.txt"),
        "hello world\nupdated content\n",
      );

      // Wait for background watcher to detect file modification and update git status
      await sleep(WATCHER_SETTLE_MS);

      const hello = findFile(finder, "hello.txt");
      expect(hello).toBeDefined();
      expect(hello?.gitStatus).toBe("modified");
    });

    test("staging a modification changes status to 'staged_modified'", async () => {
      git(tmpDir, "add", "hello.txt");

      // Wait for background watcher to detect .git/index change
      await sleep(WATCHER_SETTLE_MS);

      const hello = findFile(finder, "hello.txt");
      expect(hello).toBeDefined();
      expect(hello?.gitStatus).toBe("staged_modified");
    });

    test("committing the modification returns to 'clean'", async () => {
      git(tmpDir, "commit", "-m", "update hello");

      // Wait for background watcher to detect .git changes
      await sleep(WATCHER_SETTLE_MS);

      const hello = findFile(finder, "hello.txt");
      expect(hello).toBeDefined();
      expect(hello?.gitStatus).toBe("clean");
    });

    test("deleting a file removes it from the index", async () => {
      unlinkSync(join(tmpDir, "new_file.ts"));

      await sleep(WATCHER_SETTLE_MS);

      const result = finder.search("new_file.ts", { pageSize: 200 });
      expect(result.ok).toBe(true);
      if (!result.ok) return;

      const found = result.value.items.find(
        (i) => i.fileName === "new_file.ts",
      );
      expect(found).toBeUndefined();

      // Total should be back to 3
      const all = finder.search("", { pageSize: 200 });
      expect(all.ok).toBe(true);
      if (all.ok) {
        expect(all.value.totalFiles).toBe(3);
      }
    });

    test("adding a file in a subdirectory works", async () => {
      writeFileSync(join(tmpDir, "src", "utils.rs"), "pub fn helper() {}\n");

      // Wait for background watcher to detect new file and update git status
      await sleep(WATCHER_SETTLE_MS);

      const utils = findFile(finder, "utils.rs");
      expect(utils).toBeDefined();
      expect(utils?.relativePath).toBe("src/utils.rs");
      expect(utils?.gitStatus).toBe("untracked");
    });

    test("live grep finds content in a newly added file", async () => {
      writeFileSync(
        join(tmpDir, "src", "searchtarget.rs"),
        'const UNIQUE_NEEDLE: &str = "xylophone_waterfall_97";\n',
      );

      await sleep(WATCHER_SETTLE_MS);

      const result = finder.liveGrep("xylophone_waterfall_97", {
        mode: "plain",
      });
      expect(result.ok).toBe(true);
      if (!result.ok) return;

      expect(result.value.totalMatched).toBeGreaterThan(0);
      const match = result.value.items.find(
        (m) => m.relativePath === "src/searchtarget.rs",
      );
      expect(match).toBeDefined();
      expect(match!.lineContent).toContain("xylophone_waterfall_97");
    });

    test("live grep no longer finds content after file is deleted", async () => {
      unlinkSync(join(tmpDir, "src", "searchtarget.rs"));

      await sleep(WATCHER_SETTLE_MS);

      const result = finder.liveGrep("xylophone_waterfall_97", {
        mode: "plain",
      });
      expect(result.ok).toBe(true);
      if (!result.ok) return;

      expect(result.value.totalMatched).toBe(0);
      expect(result.value.items.length).toBe(0);
    });

    test("full add-commit cycle for subdirectory file", async () => {
      git(tmpDir, "add", "src/utils.rs");

      await sleep(WATCHER_SETTLE_MS);

      let utils = findFile(finder, "utils.rs");
      expect(utils).toBeDefined();
      expect(utils?.gitStatus).toBe("staged_new");

      git(tmpDir, "commit", "-m", "add utils");

      // Wait for background watcher to detect .git changes
      await sleep(WATCHER_SETTLE_MS);

      utils = findFile(finder, "utils.rs");
      expect(utils).toBeDefined();
      expect(utils?.gitStatus).toBe("clean");
    });
  },
);
