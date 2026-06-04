#!/usr/bin/env bun
/**
 * Glob benchmark: fff.glob vs Bun.Glob vs npm `glob`.
 *
 * Each engine is asked to enumerate files in a directory matching the same
 * pattern. We measure wall-clock time + result count. fff scans + indexes
 * once on init; the subsequent glob call is a filter over the in-memory
 * index — that's what we time.
 *
 * Usage:
 *   bun examples/glob-bench.ts [dir] [pattern] [iterations]
 *
 *   dir         default: cwd
 *   pattern     default: "**\/*.ts"
 *   iterations  default: 5  (each engine runs N times, best+median reported)
 *
 * Install npm glob first:
 *   bun add glob
 */

import { performance } from "node:perf_hooks";
import { resolve } from "node:path";
import { Glob as BunGlob } from "bun";
import { FileFinder } from "../src/index";

// npm glob — optional. Skip silently if not installed.
let npmGlob:
  | ((pattern: string, opts: { cwd: string }) => Promise<string[]>)
  | null = null;
try {
  const mod: {
    glob: (pattern: string, opts: { cwd: string }) => Promise<string[]>;
  } =
    // @ts-ignore - optional peer; resolved at runtime, may be absent
    await import("glob");
  npmGlob = mod.glob;
} catch {
  console.warn("npm `glob` not installed — skipping. Run: bun add glob");
}

const dir = resolve(process.argv[2] ?? process.cwd());
const pattern = process.argv[3] ?? "**/lua/**/*.lua";
const iterations = Number(process.argv[4] ?? 5);

console.log(`dir:        ${dir}`);
console.log(`pattern:    ${pattern}`);
console.log(`iterations: ${iterations}\n`);

interface Sample {
  ms: number;
  count: number;
}

function summarize(label: string, samples: Sample[]): void {
  if (samples.length === 0) {
    console.log(`${label.padEnd(16)} skipped`);
    return;
  }
  const sorted = [...samples].sort((a, b) => a.ms - b.ms);
  const best = sorted[0]!;
  const median = sorted[Math.floor(sorted.length / 2)]!;
  const worst = sorted[sorted.length - 1]!;
  const counts = new Set(samples.map((s) => s.count));
  const countStr =
    counts.size === 1 ? `${best.count}` : `[${[...counts].join(", ")}]`;
  console.log(
    `${label.padEnd(16)} best=${best.ms.toFixed(2)}ms  median=${median.ms.toFixed(2)}ms  worst=${worst.ms.toFixed(2)}ms  count=${countStr}`,
  );
}

async function bench<T>(
  fn: () => Promise<T> | T,
): Promise<{ ms: number; result: T }> {
  const start = performance.now();
  const result = await fn();
  return { ms: performance.now() - start, result };
}

// ---------------------------------------------------------------------------
// fff: init + warm scan, then time only the .glob() call. Init cost is
// reported separately because it's amortized across many subsequent calls.
// ---------------------------------------------------------------------------
const fffInit = await bench(() => {
  const result = FileFinder.create({
    basePath: dir,
    disableMmapCache: true,
    disableContentIndexing: true,
    disableWatch: true,
  });
  if (!result.ok) throw new Error(result.error);
  return result.value;
});
const finder = fffInit.result;

// Wait until initial scan done so the first .glob() doesn't see a partial
// index. Returns true = completed, false = timed out.
const scanReady = finder.waitForScanBlocking(30_000);
if (!scanReady.ok || !scanReady.value) {
  console.error("fff: initial scan did not finish in 30s — exiting");
  process.exit(1);
}
console.log(`fff init+scan: ${fffInit.ms.toFixed(2)}ms\n`);

const fffSamples: Sample[] = [];
for (let i = 0; i < iterations; i++) {
  const r = await bench(() => {
    const out = finder.glob(pattern, { pageSize: 100 });
    if (!out.ok) throw new Error(out.error);
    return out.value;
  });
  fffSamples.push({ ms: r.ms, count: r.result.items.length });
}

// ---------------------------------------------------------------------------
// Bun.Glob — sync iterator, returns relative paths.
// ---------------------------------------------------------------------------
const bunSamples: Sample[] = [];
for (let i = 0; i < iterations; i++) {
  const r = await bench(() => {
    const g = new BunGlob(pattern);
    let count = 0;
    for (const _ of g.scanSync({ cwd: dir })) count++;
    return count;
  });
  bunSamples.push({ ms: r.ms, count: r.result });
}

// ---------------------------------------------------------------------------
// npm glob — async, returns absolute or relative paths depending on opts.
// ---------------------------------------------------------------------------
const npmSamples: Sample[] = [];
if (npmGlob) {
  for (let i = 0; i < iterations; i++) {
    const r = await bench(() => npmGlob!(pattern, { cwd: dir }));
    npmSamples.push({ ms: r.ms, count: r.result.length });
  }
}

console.log("results:");
summarize("fff.glob", fffSamples);
summarize("Bun.Glob", bunSamples);
summarize("npm glob", npmSamples);

// Sanity: counts should be in the same ballpark. They won't match exactly
// because indexing rules differ (fff respects gitignore + skips binaries by
// default; Bun.Glob and npm glob do not).
const counts = {
  fff: fffSamples[0]?.count ?? 0,
  bun: bunSamples[0]?.count ?? 0,
  npm: npmSamples[0]?.count ?? 0,
};
console.log(
  `\nNote: fff respects gitignore + skips binaries; Bun.Glob and npm glob walk the raw filesystem. Count differences are expected.`,
);
console.log(
  `raw counts: fff=${counts.fff} bun=${counts.bun} npm=${counts.npm}`,
);

finder.destroy();
