/**
 * Stress reproducer for issue #515.
 *
 * Repeatedly creates a FileFinder, waits for scan, runs mixed file / dir /
 * grep operations with periodic scanFiles() and refreshGitStatus() calls,
 * destroys it, then repeats across two repos.
 *
 * Usage:
 *   node test/stress-515.mjs [iterations] [repoA] [repoB]
 */

import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { FileFinder } from "../dist/src/index.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, "..", "..", "..");

const args = process.argv.slice(2);
const ITERS = Number(args[0] || process.env.FFF_STRESS_ITERS || 50);
const REPO_A = resolve(args[1] || process.env.FFF_STRESS_REPO_A || REPO_ROOT);
const REPO_B_CANDIDATE = args[2] || process.env.FFF_STRESS_REPO_B || resolve(REPO_ROOT, "big-repo");
const REPO_B = existsSync(REPO_B_CANDIDATE) ? resolve(REPO_B_CANDIDATE) : REPO_A;

const REPOS = REPO_A === REPO_B ? [REPO_A] : [REPO_A, REPO_B];

const SEARCH_QUERIES = [
  "main",
  "lib",
  "fn",
  "TODO",
  "config",
  "README",
  "Cargo",
  "test",
  "src/",
  "init",
  "pub",
  "use",
];

const GREP_QUERIES = [
  "fn ",
  "pub fn",
  "TODO",
  "FIXME",
  "use std",
  "impl ",
  "struct ",
  "let mut",
  "return",
  "match ",
];

function pick(arr) {
  return arr[Math.floor(Math.random() * arr.length)];
}

async function runIteration(iter) {
  const base = REPOS[iter % REPOS.length];
  process.stdout.write(`[iter ${iter}] base=${base} `);

  const created = FileFinder.create({
    basePath: base,
    logFilePath: `/tmp/fff-515-${iter}.log`,
    logLevel: "debug",
  });
  if (!created.ok) {
    console.error(`create failed: ${created.error}`);
    return false;
  }
  const finder = created.value;

  const wait = await finder.waitForScan(60_000);
  if (!wait.ok || !wait.value) {
    console.error(`waitForScan failed: ${JSON.stringify(wait)}`);
    finder.destroy();
    return false;
  }

  process.stdout.write("scan-done ");

  const opCount = 30 + Math.floor(Math.random() * 30);
  for (let i = 0; i < opCount; i++) {
    const r = Math.random();
    if (r < 0.35) {
      finder.fileSearch(pick(SEARCH_QUERIES), { pageSize: 20 });
    } else if (r < 0.55) {
      finder.directorySearch(pick(SEARCH_QUERIES), { pageSize: 20 });
    } else if (r < 0.85) {
      finder.grep(pick(GREP_QUERIES), { mode: "plain", pageSize: 20 });
    } else if (r < 0.93) {
      finder.scanFiles();
    } else {
      finder.refreshGitStatus();
    }
  }

  process.stdout.write("ops-done ");

  finder.destroy();
  process.stdout.write("destroyed\n");
  return true;
}

(async () => {
  console.log(`Running ${ITERS} iterations across:`);
  console.log(`  A: ${REPO_A}`);
  console.log(`  B: ${REPO_B}`);

  for (let i = 0; i < ITERS; i++) {
    const ok = await runIteration(i);
    if (!ok) {
      console.error(`Aborting at iteration ${i}`);
      process.exit(1);
    }
  }

  console.log("Completed without crash.");
})();
