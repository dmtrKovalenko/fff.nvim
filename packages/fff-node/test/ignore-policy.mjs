import { strict as assert } from "node:assert";
import { execFileSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";
import { FileFinder } from "../dist/src/index.js";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitFor(predicate, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = predicate();
    if (value) return value;
    await sleep(50);
  }
  return predicate();
}

let repoDir = "";
let globalIgnore = "";
let finder = null;

function indexedPaths() {
  const result = finder.fileSearch("", { pageSize: 100 });
  assert.ok(result.ok, `search failed: ${!result.ok ? result.error : ""}`);
  return new Set(result.value.items.map((item) => item.relativePath));
}

describe("fff-node Git ignore policy", { concurrency: 1 }, () => {
  before(async () => {
    repoDir = mkdtempSync(join(tmpdir(), "fff-ignore-policy-"));
    globalIgnore = join(tmpdir(), `fff-global-ignore-${process.pid}`);
    execFileSync("git", ["init", "--quiet", repoDir]);
    execFileSync("git", ["-C", repoDir, "config", "core.excludesFile", globalIgnore]);

    mkdirSync(join(repoDir, "nested"));
    mkdirSync(join(repoDir, ".git", "info"), { recursive: true });
    writeFileSync(globalIgnore, "*.tmp\n");
    writeFileSync(join(repoDir, ".git", "info", "exclude"), "info-only.txt\n");
    writeFileSync(join(repoDir, ".gitignore"), "!kept.tmp\nnested/*.log\n");
    writeFileSync(join(repoDir, "global.tmp"), "ignored by the global policy\n");
    writeFileSync(join(repoDir, "kept.tmp"), "root negation wins\n");
    writeFileSync(join(repoDir, "info-only.txt"), "ignored by info/exclude\n");
    writeFileSync(join(repoDir, "visible.md"), "visible\n");
    writeFileSync(join(repoDir, "nested", "ignored.log"), "ignored by root\n");

    const result = FileFinder.create({ basePath: repoDir });
    assert.ok(result.ok, `create failed: ${!result.ok ? result.error : ""}`);
    finder = result.value;
    const scanned = await finder.waitForScan(10_000);
    assert.ok(scanned.ok && scanned.value, "initial scan should finish");
    const watcherReady = await waitFor(() => {
      const progress = finder.getScanProgress();
      return progress.ok && progress.value.isWatcherReady;
    });
    assert.ok(watcherReady, "watcher should become ready");
  });

  after(() => {
    if (finder && !finder.isDestroyed) finder.destroy();
    if (repoDir) rmSync(repoDir, { recursive: true, force: true });
    if (globalIgnore) rmSync(globalIgnore, { force: true });
  });

  it("applies global, info, root, and nested precedence in the Node binding", () => {
    const paths = indexedPaths();
    assert.ok(paths.has("kept.tmp"));
    assert.ok(paths.has("visible.md"));
    assert.ok(!paths.has("global.tmp"));
    assert.ok(!paths.has("info-only.txt"));
    assert.ok(!paths.has("nested/ignored.log"));
  });

  it("rescans when an external policy source changes", async () => {
    const events = [];
    const subscription = finder.watch(repoDir, (batch) => events.push(...batch));
    assert.ok(subscription.ok, `watch failed: ${!subscription.ok ? subscription.error : ""}`);

    writeFileSync(globalIgnore, "*.bak\n");
    const rescan = await waitFor(() => events.some((event) => event.kind === "rescan"));
    assert.ok(rescan, `expected rescan event, got ${JSON.stringify(events)}`);

    const policyApplied = await waitFor(() => indexedPaths().has("global.tmp"));
    assert.ok(policyApplied, "the rebuilt index should use the changed global policy");
    subscription.value();
  });
});
