#!/usr/bin/env bun
/**
 * Interactive file finder demo
 *
 * Usage:
 *   bunx fff-demo [directory]
 *   bun examples/search.ts [directory]
 *
 * Indexes the specified directory (or cwd) and provides an interactive
 * search prompt with detailed metadata about results.
 *
 * Modes:
 *   :files       - fuzzy file search (default)
 *   :dirs        - fuzzy directory search
 *   :mixed       - mixed file + directory search
 */

import { FileFinder } from "../src/index";
import type { DirItem, FileItem, MixedItem, Score } from "../src/index";
import * as readline from "node:readline";
import { join } from "node:path";
import { homedir } from "node:os";

const RESET = "\x1b[0m";
const BOLD = "\x1b[1m";
const DIM = "\x1b[2m";
const GREEN = "\x1b[32m";
const YELLOW = "\x1b[33m";
const BLUE = "\x1b[34m";
const CYAN = "\x1b[36m";
const MAGENTA = "\x1b[35m";
const RED = "\x1b[31m";

type SearchMode = "files" | "dirs" | "mixed";

function formatGitStatus(status: string): string {
  switch (status) {
    case "modified":
      return `${YELLOW}M${RESET}`;
    case "untracked":
      return `${GREEN}?${RESET}`;
    case "added":
      return `${GREEN}A${RESET}`;
    case "deleted":
      return `${RED}D${RESET}`;
    case "renamed":
      return `${BLUE}R${RESET}`;
    case "clean":
    case "current":
      return `${DIM} ${RESET}`;
    default:
      return `${DIM}${status.charAt(0)}${RESET}`;
  }
}

function formatScore(score: number): string {
  const s = String(score);
  if (score >= 100) return `${GREEN}${s}${RESET}`;
  if (score >= 50) return `${YELLOW}${s}${RESET}`;
  if (score > 0) return `${DIM}${s}${RESET}`;
  return `${DIM}0${RESET}`;
}

/** Pad a plain string first, then wrap with ANSI color. */
function padColor(
  value: string,
  width: number,
  color: string,
  align: "left" | "right" = "right",
): string {
  const padded = align === "right" ? value.padStart(width) : value.padEnd(width);
  return `${color}${padded}${RESET}`;
}

function scoreColor(score: number): string {
  if (score >= 100) return GREEN;
  if (score >= 50) return YELLOW;
  return DIM;
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}K`;
  return `${(bytes / 1024 / 1024).toFixed(1)}M`;
}

function formatTime(unixSeconds: number): string {
  if (unixSeconds === 0) return "unknown";
  const date = new Date(unixSeconds * 1000);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  const diffHours = Math.floor(diffMs / 3600000);
  const diffDays = Math.floor(diffMs / 86400000);

  if (diffMins < 1) return "just now";
  if (diffMins < 60) return `${diffMins}m ago`;
  if (diffHours < 24) return `${diffHours}h ago`;
  if (diffDays < 7) return `${diffDays}d ago`;
  return date.toLocaleDateString();
}

function formatModeTag(mode: SearchMode): string {
  switch (mode) {
    case "files":
      return `${CYAN}files${RESET}`;
    case "dirs":
      return `${MAGENTA}dirs${RESET}`;
    case "mixed":
      return `${YELLOW}mixed${RESET}`;
  }
}

function printScoreBreakdown(score: Score, indent: string) {
  const breakdown: string[] = [];
  if (score.baseScore > 0) breakdown.push(`base:${score.baseScore}`);
  if (score.filenameBonus > 0) breakdown.push(`filename:+${score.filenameBonus}`);
  if (score.frecencyBoost > 0) breakdown.push(`frecency:+${score.frecencyBoost}`);
  if (score.comboMatchBoost > 0) breakdown.push(`combo:+${score.comboMatchBoost}`);
  if (score.distancePenalty < 0) breakdown.push(`distance:${score.distancePenalty}`);
  if (score.exactMatch) breakdown.push(`${GREEN}exact${RESET}`);
  if (score.matchType) breakdown.push(`[${score.matchType}]`);

  if (breakdown.length > 0) {
    console.log(`${DIM}${indent}└─ ${breakdown.join(", ")}${RESET}`);
  }
}

function printFileResult(item: FileItem, score: Score, showBreakdown: boolean) {
  const git = formatGitStatus(item.gitStatus);
  const sc = padColor(String(score.total), 5, scoreColor(score.total));
  const size = formatSize(item.size).padStart(6);
  const modified = formatTime(item.modified).padEnd(10);

  console.log(`   ${git}  │ ${sc} │ ${size} │ ${modified} │ ${item.relativePath}`);

  if (showBreakdown && score.total > 0) {
    printScoreBreakdown(score, "      │       │        │            │  ");
  }
}

function printDirResult(item: DirItem, score: Score, showBreakdown: boolean) {
  const sc = padColor(String(score.total), 5, scoreColor(score.total));
  const frecency = (
    item.maxAccessFrecency > 0 ? `f:${item.maxAccessFrecency}` : "-"
  ).padStart(6);

  console.log(
    `   ${MAGENTA}D${RESET}  │ ${sc} │ ${frecency} │ ${"".padEnd(10)} │ ${MAGENTA}${item.relativePath}${RESET}`,
  );

  if (showBreakdown && score.total > 0) {
    printScoreBreakdown(score, "      │       │        │            │  ");
  }
}

function printMixedResult(mixed: MixedItem, score: Score, showBreakdown: boolean) {
  if (mixed.type === "file") {
    printFileResult(mixed.item, score, showBreakdown);
  } else {
    printDirResult(mixed.item, score, showBreakdown);
  }
}

async function main() {
  const targetDir = process.argv[2] || process.cwd();

  console.log(`${BOLD}${CYAN}fff - Fast File Finder Demo${RESET}\n`);

  // Check library availability
  if (!FileFinder.isAvailable()) {
    console.error(`${RED}Error: Native library not found.${RESET}`);
    console.error("Build with: cargo build --release -p fff-c");
    process.exit(1);
  }

  // Use the same frecency + history databases as the Neovim plugin
  // so the demo benefits from real access history.
  const nvimCache = process.env.XDG_CACHE_HOME || join(homedir(), ".cache", "nvim");
  const nvimData =
    process.env.XDG_DATA_HOME || join(homedir(), ".local", "share", "nvim");
  const frecencyDbPath = join(nvimCache, "fff_nvim");
  const historyDbPath = join(nvimData, "fff_queries");

  console.log(`${DIM}Initializing index for: ${targetDir}${RESET}`);
  console.log(`${DIM}Frecency DB: ${frecencyDbPath}${RESET}`);
  const createResult = FileFinder.create({
    basePath: targetDir,
    frecencyDbPath,
    historyDbPath,
    useUnsafeNoLock: true,
  });

  if (!createResult.ok) {
    console.error(`${RED}Init failed: ${createResult.error}${RESET}`);
    process.exit(1);
  }

  const finder = createResult.value;

  // Wait for scan with progress
  process.stdout.write(`${DIM}Scanning files...${RESET}`);
  const startTime = Date.now();
  let lastCount = 0;

  while (finder.isScanning()) {
    const progress = finder.getScanProgress();
    if (progress.ok && progress.value.scannedFilesCount !== lastCount) {
      lastCount = progress.value.scannedFilesCount;
      process.stdout.write(`\r${DIM}Scanning files... ${lastCount}${RESET}   `);
    }
    await new Promise((r) => setTimeout(r, 50));
  }

  const scanTime = Date.now() - startTime;
  const finalProgress = finder.getScanProgress();
  const totalFiles = finalProgress.ok ? finalProgress.value.scannedFilesCount : 0;

  console.log(
    `\r${GREEN}✓${RESET} Indexed ${BOLD}${totalFiles}${RESET} files in ${scanTime}ms\n`,
  );

  // Show index info
  const health = finder.healthCheck();
  if (health.ok) {
    console.log(`${DIM}Version:${RESET}    ${health.value.version}`);
    console.log(`${DIM}Base path:${RESET}  ${health.value.filePicker.basePath}`);
    if (health.value.git.repositoryFound) {
      console.log(`${DIM}Git root:${RESET}   ${health.value.git.workdir}`);
    }
  }

  let mode: SearchMode = "files";

  // Interactive search loop
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });

  console.log(`\n${BOLD}Commands:${RESET}`);
  console.log(`  ${DIM}:files${RESET}  - file search mode (default)`);
  console.log(`  ${DIM}:dirs${RESET}   - directory search mode`);
  console.log(`  ${DIM}:mixed${RESET}  - mixed files + directories mode`);
  console.log(`  ${DIM}q${RESET}       - quit`);
  console.log();

  const prompt = () => {
    rl.question(`${CYAN}[${formatModeTag(mode)}]>${RESET} `, (query) => {
      const trimmed = query.trim();

      if (trimmed.toLowerCase() === "q" || trimmed.toLowerCase() === "quit") {
        console.log(`\n${DIM}Goodbye!${RESET}`);
        finder.destroy();
        rl.close();
        process.exit(0);
      }

      // Mode switching commands
      if (trimmed === ":files") {
        mode = "files";
        console.log(`${GREEN}Switched to file search mode${RESET}\n`);
        prompt();
        return;
      }
      if (trimmed === ":dirs") {
        mode = "dirs";
        console.log(`${MAGENTA}Switched to directory search mode${RESET}\n`);
        prompt();
        return;
      }
      if (trimmed === ":mixed") {
        mode = "mixed";
        console.log(`${YELLOW}Switched to mixed search mode${RESET}\n`);
        prompt();
        return;
      }

      const searchStart = Date.now();

      if (mode === "files") {
        const result = finder.fileSearch(trimmed, { pageSize: 15 });
        const searchTime = Date.now() - searchStart;

        if (!result.ok) {
          console.log(`${RED}Search error: ${result.error}${RESET}\n`);
          prompt();
          return;
        }

        const { items, scores, totalMatched, totalFiles } = result.value;

        console.log();
        console.log(
          `${DIM}Found ${BOLD}${totalMatched}${RESET}${DIM} matches in ${totalFiles} files (${searchTime}ms)${RESET}`,
        );
        console.log();

        if (items.length === 0) {
          console.log(`${DIM}No matches found.${RESET}\n`);
          prompt();
          return;
        }

        console.log(`${DIM}  Git │ Score │  Size  │  Modified  │ Path${RESET}`);

        for (let i = 0; i < items.length; i++) {
          printFileResult(items[i], scores[i], i < 3);
        }

        if (totalMatched > items.length) {
          console.log(
            `${DIM}      │       │        │            │ ... and ${totalMatched - items.length} more${RESET}`,
          );
        }
      } else if (mode === "dirs") {
        const result = finder.directorySearch(trimmed, { pageSize: 15 });
        const searchTime = Date.now() - searchStart;

        if (!result.ok) {
          console.log(`${RED}Search error: ${result.error}${RESET}\n`);
          prompt();
          return;
        }

        const { items, scores, totalMatched, totalDirs } = result.value;

        console.log();
        console.log(
          `${DIM}Found ${BOLD}${totalMatched}${RESET}${DIM} matches in ${totalDirs} directories (${searchTime}ms)${RESET}`,
        );
        console.log();

        if (items.length === 0) {
          console.log(`${DIM}No matches found.${RESET}\n`);
          prompt();
          return;
        }

        console.log(`${DIM}  Dir │ Score │ Frecnc │            │ Path${RESET}`);

        for (let i = 0; i < items.length; i++) {
          printDirResult(items[i], scores[i], i < 3);
        }

        if (totalMatched > items.length) {
          console.log(
            `${DIM}      │       │        │            │ ... and ${totalMatched - items.length} more${RESET}`,
          );
        }
      } else {
        // mixed mode
        const result = finder.mixedSearch(trimmed, { pageSize: 20 });
        const searchTime = Date.now() - searchStart;

        if (!result.ok) {
          console.log(`${RED}Search error: ${result.error}${RESET}\n`);
          prompt();
          return;
        }

        const { items, scores, totalMatched, totalFiles, totalDirs } = result.value;

        console.log();
        console.log(
          `${DIM}Found ${BOLD}${totalMatched}${RESET}${DIM} matches (${totalFiles} files, ${totalDirs} dirs) (${searchTime}ms)${RESET}`,
        );
        console.log();

        if (items.length === 0) {
          console.log(`${DIM}No matches found.${RESET}\n`);
          prompt();
          return;
        }

        console.log(`${DIM} Type │ Score │  Info  │  Modified  │ Path${RESET}`);

        for (let i = 0; i < items.length; i++) {
          printMixedResult(items[i], scores[i], i < 5);
        }

        if (totalMatched > items.length) {
          console.log(
            `${DIM}      │       │        │            │ ... and ${totalMatched - items.length} more${RESET}`,
          );
        }
      }

      console.log();
      prompt();
    });
  };

  prompt();
}

main().catch((err) => {
  console.error(`${RED}Fatal error: ${err.message}${RESET}`);
  process.exit(1);
});
