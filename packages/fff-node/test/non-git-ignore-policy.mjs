import { strict as assert } from "node:assert";
import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, it } from "node:test";

const scenario = process.env.FFF_NON_GIT_IGNORE_SCENARIO;
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

async function runScenario(kind) {
  const fixture = mkdtempSync(join(tmpdir(), `fff-non-git-${kind}-`));
  const vault = join(fixture, "vault");
  const configHome = join(fixture, "config");
  const home = join(fixture, "home");
  const gitConfigDir = join(configHome, "git");
  const defaultIgnore = join(gitConfigDir, "ignore");
  const configuredIgnore = join(fixture, "configured-ignore");
  const policySource = kind === "configured" ? configuredIgnore : defaultIgnore;

  mkdirSync(vault);
  mkdirSync(home);
  mkdirSync(gitConfigDir, { recursive: true });
  if (kind === "configured") {
    writeFileSync(
      join(gitConfigDir, "config"),
      `[core]\n\texcludesFile = ${configuredIgnore}\n`,
    );
  }
  writeFileSync(policySource, "*.tmp\n");
  writeFileSync(join(vault, "global.tmp"), "ignored\n");
  writeFileSync(join(vault, "visible.md"), "visible\n");

  process.env.XDG_CONFIG_HOME = configHome;
  process.env.HOME = home;

  const { FileFinder } = await import("../dist/src/index.js");
  let finder = null;
  try {
    const created = FileFinder.create({ basePath: vault });
    assert.ok(created.ok, `create failed: ${!created.ok ? created.error : ""}`);
    finder = created.value;
    const scanned = await finder.waitForScan(10_000);
    assert.ok(scanned.ok && scanned.value, "initial scan should finish");
    const watcherReady = await waitFor(() => {
      const progress = finder.getScanProgress();
      return progress.ok && progress.value.isWatcherReady;
    });
    assert.ok(watcherReady, "watcher should become ready");

    const initial = finder.fileSearch("", { pageSize: 100 });
    assert.ok(initial.ok);
    const initialPaths = new Set(initial.value.items.map((item) => item.relativePath));
    assert.ok(!initialPaths.has("global.tmp"), `${kind} policy was not applied`);
    assert.ok(initialPaths.has("visible.md"));

    const events = [];
    const subscription = finder.watch(vault, (batch) => events.push(...batch));
    assert.ok(subscription.ok, `watch failed: ${!subscription.ok ? subscription.error : ""}`);
    writeFileSync(policySource, "*.bak\n");

    const rescanned = await waitFor(() => events.some((event) => event.kind === "rescan"));
    assert.ok(rescanned, `${kind} policy change did not emit rescan`);
    const applied = await waitFor(() => {
      const result = finder.fileSearch("", { pageSize: 100 });
      return result.ok && result.value.items.some((item) => item.relativePath === "global.tmp");
    });
    assert.ok(applied, `${kind} policy change did not rebuild the index`);
    subscription.value();
  } finally {
    if (finder && !finder.isDestroyed) finder.destroy();
    rmSync(fixture, { recursive: true, force: true });
  }
}

if (scenario) {
  await runScenario(scenario);
} else {
  describe("fff-node non-Git vault ignore policy", { concurrency: 1 }, () => {
    for (const kind of ["configured", "default"]) {
      it(`applies and watches ${kind} user excludes`, () => {
        const result = spawnSync(process.execPath, [fileURLToPath(import.meta.url)], {
          env: { ...process.env, FFF_NON_GIT_IGNORE_SCENARIO: kind },
          encoding: "utf8",
          timeout: 20_000,
        });
        assert.equal(
          result.status,
          0,
          `child failed (${result.signal ?? "no signal"}):\n${result.stdout}\n${result.stderr}`,
        );
      });
    }
  });
}
