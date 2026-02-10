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
 */

import { FileFinder } from "../src/index";
import * as readline from "readline";

const RESET = "\x1b[0m";
const BOLD = "\x1b[1m";
const DIM = "\x1b[2m";
const GREEN = "\x1b[32m";
const YELLOW = "\x1b[33m";
const BLUE = "\x1b[34m";
const MAGENTA = "\x1b[35m";
const CYAN = "\x1b[36m";
const RED = "\x1b[31m";

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
    case "clear":
    case "current":
      return `${DIM} ${RESET}`;
    default:
      return `${DIM}${status.charAt(0)}${RESET}`;
  }
}

function formatScore(score: number): string {
  if (score >= 100) return `${GREEN}${score}${RESET}`;
  if (score >= 50) return `${YELLOW}${score}${RESET}`;
  if (score > 0) return `${DIM}${score}${RESET}`;
  return `${DIM}0${RESET}`;
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

async function main() {
  const targetDir = process.argv[2] || process.cwd();
  
  console.log(`${BOLD}${CYAN}fff - Fast File Finder Demo${RESET}\n`);

  // Check library availability
  if (!FileFinder.isAvailable()) {
    console.error(`${RED}Error: Native library not found.${RESET}`);
    console.error("Build with: cargo build --release -p fff-c");
    process.exit(1);
  }

  // Initialize
  console.log(`${DIM}Initializing index for: ${targetDir}${RESET}`);
  const initResult = FileFinder.init({
    basePath: targetDir,
    skipDatabases: true, // Skip frecency DB for demo simplicity
  });

  if (!initResult.ok) {
    console.error(`${RED}Init failed: ${initResult.error}${RESET}`);
    process.exit(1);
  }

  // Wait for scan with progress
  process.stdout.write(`${DIM}Scanning files...${RESET}`);
  const startTime = Date.now();
  let lastCount = 0;

  while (FileFinder.isScanning()) {
    const progress = FileFinder.getScanProgress();
    if (progress.ok && progress.value.scannedFilesCount !== lastCount) {
      lastCount = progress.value.scannedFilesCount;
      process.stdout.write(`\r${DIM}Scanning files... ${lastCount}${RESET}   `);
    }
    await new Promise((r) => setTimeout(r, 50));
  }

  const scanTime = Date.now() - startTime;
  const finalProgress = FileFinder.getScanProgress();
  const totalFiles = finalProgress.ok ? finalProgress.value.scannedFilesCount : 0;

  console.log(`\r${GREEN}✓${RESET} Indexed ${BOLD}${totalFiles}${RESET} files in ${scanTime}ms\n`);

  // Show index info
  const health = FileFinder.healthCheck();
  if (health.ok) {
    console.log(`${DIM}─────────────────────────────────────────${RESET}`);
    console.log(`${DIM}Version:${RESET}    ${health.value.version}`);
    console.log(`${DIM}Base path:${RESET}  ${health.value.filePicker.basePath}`);
    if (health.value.git.repositoryFound) {
      console.log(`${DIM}Git root:${RESET}   ${health.value.git.workdir}`);
    }
    console.log(`${DIM}─────────────────────────────────────────${RESET}\n`);
  }

  // Interactive search loop
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });

  console.log(`${BOLD}Enter a search query${RESET} (or 'q' to quit, empty for all files):\n`);

  const prompt = () => {
    rl.question(`${CYAN}search>${RESET} `, (query) => {
      if (query.toLowerCase() === "q" || query.toLowerCase() === "quit") {
        console.log(`\n${DIM}Goodbye!${RESET}`);
        FileFinder.destroy();
        rl.close();
        process.exit(0);
      }

      const searchStart = Date.now();
      const result = FileFinder.search(query, { pageSize: 15 });
      const searchTime = Date.now() - searchStart;

      if (!result.ok) {
        console.log(`${RED}Search error: ${result.error}${RESET}\n`);
        prompt();
        return;
      }

      const { items, scores, totalMatched, totalFiles } = result.value;

      console.log();
      console.log(
        `${DIM}Found ${BOLD}${totalMatched}${RESET}${DIM} matches in ${totalFiles} files (${searchTime}ms)${RESET}`
      );
      console.log();

      if (items.length === 0) {
        console.log(`${DIM}No matches found.${RESET}\n`);
        prompt();
        return;
      }

      // Header
      console.log(
        `${DIM}  Git │ Score │  Size  │  Modified  │ Path${RESET}`
      );
      console.log(`${DIM}──────┼───────┼────────┼────────────┼${"─".repeat(40)}${RESET}`);

      // Results
      for (let i = 0; i < items.length; i++) {
        const item = items[i];
        const score = scores[i];

        const gitStatus = formatGitStatus(item.gitStatus);
        const totalScore = formatScore(score.total);
        const size = formatSize(item.size).padStart(6);
        const modified = formatTime(item.modified).padEnd(10);
        const path = item.relativePath;

        console.log(
          `   ${gitStatus}  │ ${totalScore.padStart(5)} │ ${size} │ ${modified} │ ${path}`
        );

        // Show score breakdown for top results
        if (i < 3 && score.total > 0) {
          const breakdown: string[] = [];
          if (score.baseScore > 0) breakdown.push(`base:${score.baseScore}`);
          if (score.filenameBonus > 0) breakdown.push(`filename:+${score.filenameBonus}`);
          if (score.frecencyBoost > 0) breakdown.push(`frecency:+${score.frecencyBoost}`);
          if (score.comboMatchBoost > 0) breakdown.push(`combo:+${score.comboMatchBoost}`);
          if (score.distancePenalty < 0) breakdown.push(`distance:${score.distancePenalty}`);
          if (score.exactMatch) breakdown.push(`${GREEN}exact${RESET}`);
          
          if (breakdown.length > 0) {
            console.log(`${DIM}      │       │        │            │  └─ ${breakdown.join(", ")}${RESET}`);
          }
        }
      }

      if (totalMatched > items.length) {
        console.log(
          `${DIM}      │       │        │            │ ... and ${totalMatched - items.length} more${RESET}`
        );
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
