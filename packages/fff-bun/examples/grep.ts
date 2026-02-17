#!/usr/bin/env bun
/**
 * Interactive live grep demo
 *
 * Usage:
 *   bun examples/grep.ts [directory] [--mode=plain|regex|fuzzy]
 *
 * Indexes the specified directory (or cwd) and provides an interactive
 * content search prompt with match highlighting.
 */

import { FileFinder } from "../src/index";
import type { GrepMode } from "../src/types";
import * as readline from "readline";

const RESET = "\x1b[0m";
const BOLD = "\x1b[1m";
const DIM = "\x1b[2m";
const GREEN = "\x1b[32m";
const YELLOW = "\x1b[33m";
const BLUE = "\x1b[34m";
const CYAN = "\x1b[36m";
const RED = "\x1b[31m";
const BG_YELLOW = "\x1b[43m";
const BLACK = "\x1b[30m";

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

/**
 * Highlight match ranges within line content using ANSI escape codes.
 * The match_ranges are byte offsets into line_content.
 */
function highlightLine(content: string, ranges: [number, number][]): string {
  if (ranges.length === 0) return content;

  // Convert the string to a buffer to work with byte offsets
  const buf = Buffer.from(content, "utf-8");
  const parts: string[] = [];
  let lastEnd = 0;

  for (const [start, end] of ranges) {
    // Clamp to valid range
    const s = Math.max(0, Math.min(start, buf.length));
    const e = Math.max(s, Math.min(end, buf.length));

    if (s > lastEnd) {
      parts.push(buf.subarray(lastEnd, s).toString("utf-8"));
    }
    parts.push(
      `${BG_YELLOW}${BLACK}${buf.subarray(s, e).toString("utf-8")}${RESET}`
    );
    lastEnd = e;
  }

  if (lastEnd < buf.length) {
    parts.push(buf.subarray(lastEnd).toString("utf-8"));
  }

  return parts.join("");
}

function parseArgs(): { directory: string; mode: GrepMode } {
  let directory = process.cwd();
  let mode: GrepMode = "plain";

  for (const arg of process.argv.slice(2)) {
    if (arg.startsWith("--mode=")) {
      const m = arg.slice(7);
      if (m === "plain" || m === "regex" || m === "fuzzy") {
        mode = m;
      } else {
        console.error(`Unknown mode: ${m}. Use plain, regex, or fuzzy.`);
        process.exit(1);
      }
    } else if (!arg.startsWith("-")) {
      directory = arg;
    }
  }

  return { directory, mode };
}

async function main() {
  const { directory, mode } = parseArgs();

  console.log(`${BOLD}${CYAN}fff - Live Grep Demo${RESET}`);
  console.log(`${DIM}Mode: ${mode}${RESET}\n`);

  if (!FileFinder.isAvailable()) {
    console.error(`${RED}Error: Native library not found.${RESET}`);
    console.error("Build with: cargo build --release -p fff-c");
    process.exit(1);
  }

  console.log(`${DIM}Initializing index for: ${directory}${RESET}`);
  const createResult = FileFinder.create({
    basePath: directory,
    warmupMmapCache: true,
  });

  if (!createResult.ok) {
    console.error(`${RED}Init failed: ${createResult.error}${RESET}`);
    process.exit(1);
  }

  const finder = createResult.value;

  // Wait for scan
  process.stdout.write(`${DIM}Scanning files...${RESET}`);
  const startTime = Date.now();

  while (finder.isScanning()) {
    const progress = finder.getScanProgress();
    if (progress.ok) {
      process.stdout.write(
        `\r${DIM}Scanning files... ${progress.value.scannedFilesCount}${RESET}   `
      );
    }
    await new Promise((r) => setTimeout(r, 50));
  }

  const scanTime = Date.now() - startTime;
  const finalProgress = finder.getScanProgress();
  const totalFiles = finalProgress.ok
    ? finalProgress.value.scannedFilesCount
    : 0;

  console.log(
    `\r${GREEN}✓${RESET} Indexed ${BOLD}${totalFiles}${RESET} files in ${scanTime}ms\n`
  );

  console.log(
    `${BOLD}Enter a search pattern${RESET} (or 'q' to quit, ':mode plain|regex|fuzzy' to switch):\n`
  );
  console.log(
    `${DIM}Tip: prefix with *.ext to filter by extension, e.g. "*.ts useState"${RESET}\n`
  );

  let currentMode = mode;

  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });

  const prompt = () => {
    const modeLabel =
      currentMode === "plain" ? "txt" : currentMode === "regex" ? "re" : "fzy";

    rl.question(`${CYAN}grep[${modeLabel}]>${RESET} `, (query) => {
      if (query.toLowerCase() === "q" || query.toLowerCase() === "quit") {
        console.log(`\n${DIM}Goodbye!${RESET}`);
        finder.destroy();
        rl.close();
        process.exit(0);
      }

      // Handle mode switching
      if (query.startsWith(":mode ")) {
        const newMode = query.slice(6).trim();
        if (
          newMode === "plain" ||
          newMode === "regex" ||
          newMode === "fuzzy"
        ) {
          currentMode = newMode;
          console.log(`${DIM}Switched to ${currentMode} mode${RESET}\n`);
        } else {
          console.log(
            `${RED}Unknown mode: ${newMode}. Use plain, regex, or fuzzy.${RESET}\n`
          );
        }
        prompt();
        return;
      }

      if (query.trim() === "") {
        prompt();
        return;
      }

      const searchStart = Date.now();
      const result = finder.liveGrep(query, {
        mode: currentMode,
        pageLimit: 30,
        timeBudgetMs: 5000,
      });
      const searchTime = Date.now() - searchStart;

      if (!result.ok) {
        console.log(`${RED}Grep error: ${result.error}${RESET}\n`);
        prompt();
        return;
      }

      const {
        items,
        totalMatched,
        totalFilesSearched,
        totalFiles: indexedFiles,
        filteredFileCount,
        nextCursor,
        regexFallbackError,
      } = result.value;

      console.log();

      if (regexFallbackError) {
        console.log(
          `${YELLOW}Regex error: ${regexFallbackError} (fell back to literal match)${RESET}`
        );
      }

      console.log(
        `${DIM}${BOLD}${totalMatched}${RESET}${DIM} matches across ${totalFilesSearched}/${filteredFileCount} files (${searchTime}ms)${RESET}`
      );
      console.log();

      if (items.length === 0) {
        console.log(`${DIM}No matches found.${RESET}\n`);
        prompt();
        return;
      }

      // Group matches by file for display
      let lastFile = "";
      for (const match of items) {
        if (match.relativePath !== lastFile) {
          lastFile = match.relativePath;
          const git = formatGitStatus(match.gitStatus);
          console.log(
            `${BOLD}${BLUE}${match.relativePath}${RESET} ${git}`
          );
        }

        const lineNum = String(match.lineNumber).padStart(4);
        const highlighted = highlightLine(
          match.lineContent,
          match.matchRanges
        );

        let suffix = "";
        if (match.fuzzyScore !== undefined) {
          suffix = ` ${DIM}(score: ${match.fuzzyScore})${RESET}`;
        }

        console.log(`${DIM}${lineNum}:${RESET} ${highlighted}${suffix}`);
      }

      if (nextCursor) {
        console.log(
          `\n${DIM}... more results available${RESET}`
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
