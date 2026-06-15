/**
 * Node SDK watcher churn stress.
 *
 * Keeps the native watcher enabled while mutating the watched tree and issuing
 * searches concurrently. Useful for macOS FSEvents/debouncer crash triage.
 *
 * Usage:
 *   node test/watcher-churn-stress.mjs [iterations] [opsPerIteration]
 */

import { mkdtemp, mkdir, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { FileFinder } from "../dist/src/index.js";

const ITERATIONS = numberArg(0, "FFF_WATCHER_STRESS_ITERS", 12);
const OPS_PER_ITER = numberArg(1, "FFF_WATCHER_STRESS_OPS", 2500);
const SEARCHES_PER_ITER = numberEnv("FFF_WATCHER_STRESS_SEARCHES", OPS_PER_ITER);
const LOG_LEVEL = process.env.FFF_WATCHER_STRESS_LOG_LEVEL || "debug";

function numberArg(index, envName, fallback) {
  const raw = process.argv[index + 2] || process.env[envName];
  const parsed = Number(raw);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function numberEnv(envName, fallback) {
  const parsed = Number(process.env[envName]);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function assertOk(result, context) {
  if (!result.ok) {
    throw new Error(`${context}: ${result.error}`);
  }
  return result.value;
}

function assertTrue(result, context) {
  const value = assertOk(result, context);
  if (value !== true) {
    throw new Error(`${context}: timed out`);
  }
}

async function waitForWatcher(finder, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const progress = assertOk(finder.getScanProgress(), "getScanProgress");
    if (!progress.isScanning && progress.isWatcherReady) return;
    await sleep(50);
  }
  throw new Error(`watcher was not ready after ${timeoutMs}ms`);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function seedTree(base) {
  for (let dir = 0; dir < 20; dir++) {
    const dirPath = join(base, `dir-${dir}`);
    await mkdir(dirPath, { recursive: true });
    for (let file = 0; file < 20; file++) {
      await writeFile(
        join(dirPath, `seed-${file}.txt`),
        `seed ${dir}/${file}\nalpha beta gamma\n`,
      );
    }
  }
}

async function churn(base, iteration) {
  for (let i = 0; i < OPS_PER_ITER; i++) {
    const dirPath = join(base, `hot-${iteration}-${i % 64}`);
    const filePath = join(dirPath, `file-${i % 97}.txt`);
    const nextPath = join(dirPath, `file-${i % 97}.renamed.txt`);
    await mkdir(dirPath, { recursive: true });

    switch (i % 5) {
      case 0:
      case 1:
        await writeFile(filePath, `iter=${iteration} op=${i}\nneedle ${i % 13}\n`);
        break;
      case 2:
        await writeFile(filePath, `rename source ${iteration}/${i}\n`);
        await rename(filePath, nextPath).catch(ignoreMissing);
        break;
      case 3:
        await rm(filePath, { force: true });
        await rm(nextPath, { force: true });
        break;
      default:
        await writeFile(filePath, `touch ${Date.now()} ${i}\n`);
        break;
    }

    if (i % 100 === 0) await sleep(1);
  }
}

async function search(finder) {
  const queries = ["seed", "needle", "dir", "file", "alpha", "gamma", "hot"];
  for (let i = 0; i < SEARCHES_PER_ITER; i++) {
    const query = queries[i % queries.length];
    const result =
      i % 3 === 0
        ? finder.fileSearch(query, { pageSize: 20 })
        : i % 3 === 1
          ? finder.directorySearch(query, { pageSize: 20 })
          : finder.grep(query, { mode: "plain", pageSize: 20 });
    assertOk(result, `search ${i}`);
    if (i % 100 === 0) await sleep(1);
  }
}

function ignoreMissing(error) {
  if (error && error.code !== "ENOENT") throw error;
}

async function runIteration(iteration) {
  const base = await mkdtemp(join(tmpdir(), `fff-watcher-${iteration}-`));
  await seedTree(base);

  const logFilePath = join(tmpdir(), `fff-watcher-${process.pid}-${iteration}.log`);
  const finder = assertOk(
    FileFinder.create({ basePath: base, logFilePath, logLevel: LOG_LEVEL }),
    "FileFinder.create",
  );

  try {
    assertTrue(await finder.waitForScan(30_000), "waitForScan");
    await waitForWatcher(finder, 30_000);
    process.stdout.write(
      `[iter ${iteration}] base=${base} log=${logFilePath} ops=${OPS_PER_ITER} searches=${SEARCHES_PER_ITER}\n`,
    );

    await Promise.all([churn(base, iteration), search(finder)]);
    await sleep(500);

    const progress = assertOk(finder.getScanProgress(), "final getScanProgress");
    process.stdout.write(
      `[iter ${iteration}] done files=${progress.scannedFilesCount} watcher=${progress.isWatcherReady}\n`,
    );
  } finally {
    finder.destroy();
    await rm(base, { recursive: true, force: true });
  }
}

for (let i = 0; i < ITERATIONS; i++) {
  await runIteration(i);
}

console.log("Completed watcher churn stress without JS-level errors.");
