/**
 * Token-efficient output formatting for LLM tool results.
 * Port of crates/fff-mcp/src/output.rs
 */

import type { FileItem, GrepMatch, Score } from "@ff-labs/fff-node";
import type { CursorStore } from "./cursor-store.js";

const MAX_PREVIEW = 120;
const MAX_LINE_LEN = 180;
const MAX_DEF_EXPAND_FIRST = 8;
const MAX_DEF_EXPAND = 5;
const LARGE_FILE_BYTES = 20_000;

function frecencyWord(score: number): string | null {
  if (score >= 100) return "hot";
  if (score >= 50) return "warm";
  if (score >= 10) return "frequent";
  return null;
}

export function fileSuffix(gitStatus: string, frecencyScore: number): string {
  const f = frecencyWord(frecencyScore);
  const g =
    gitStatus && gitStatus !== "clean" && gitStatus !== "ignored" ? gitStatus : null;

  if (f && g) return ` - ${f} git:${g}`;
  if (f) return ` - ${f}`;
  if (g) return ` git:${g}`;
  return "";
}

function sizeTag(bytes: number): string {
  if (bytes < LARGE_FILE_BYTES) return "";
  const kb = Math.round((bytes + 512) / 1024);
  return ` (${kb}KB - use offset to read relevant section)`;
}

export function truncateLineForAi(
  line: string,
  matchRanges: [number, number][] | null,
  maxLen: number,
): string {
  const trimmed = line.trim();
  if (!trimmed) return "";
  if (trimmed.length <= maxLen) return trimmed;

  const stripOffset = line.length - line.trimStart().length;

  // Adjust match ranges for stripped leading whitespace
  const adjusted =
    matchRanges && stripOffset > 0
      ? matchRanges.map(
          ([s, e]) =>
            [Math.max(0, s - stripOffset), Math.max(0, e - stripOffset)] as [
              number,
              number,
            ],
        )
      : matchRanges;

  // Use first match range to center the window
  if (adjusted && adjusted.length > 0) {
    const [matchStart, matchEnd] = adjusted[0];
    const matchLen = matchEnd - matchStart;
    const budget = maxLen - matchLen;
    const before = Math.floor(budget / 3);
    const after = budget - before;

    const winStart = Math.max(0, matchStart - before);
    const winEnd = Math.min(trimmed.length, matchEnd + after);

    let result = trimmed.slice(winStart, winEnd);
    if (winStart > 0) result = `…${result}`;
    if (winEnd < trimmed.length) result = `${result}…`;
    return result;
  }

  return `${trimmed.slice(0, maxLen)}…`;
}

export function formatFindFilesResult(
  items: FileItem[],
  scores: Score[],
  totalMatched: number,
  totalFiles: number,
  pageOffset: number,
  cursorStore: CursorStore,
): string {
  if (items.length === 0) {
    return `0 results (${totalFiles} indexed)`;
  }

  const lines: string[] = [];
  const isExactMatch = scores[0]?.exactMatch ?? false;

  if (pageOffset === 0) {
    if (isExactMatch) {
      lines.push(`→ Read ${items[0].relativePath} (exact match!)`);
    } else if (scores.length < 2 || scores[0].total > scores[1].total * 2) {
      lines.push(
        `→ Read ${items[0].relativePath} (best match — Read this file directly)`,
      );
    }
  }

  const nextOffset = pageOffset + items.length;
  const hasMore = nextOffset < totalMatched;

  if (hasMore) {
    lines.push(`${items.length}/${totalMatched} matches`);
  }

  for (const item of items) {
    lines.push(
      `${item.relativePath}${fileSuffix(item.gitStatus, item.totalFrecencyScore)}`,
    );
  }

  if (hasMore) {
    const cursorId = cursorStore.store(nextOffset);
    lines.push(`cursor: ${cursorId}`);
  }

  return lines.join("\n");
}

interface FileMeta {
  relativePath: string;
  size: number;
  lineNumber: number;
  lineContent: string;
  isDefinition: boolean;
  matchRanges: [number, number][];
  contextAfter: string[];
}

function collectFilePreview(matches: GrepMatch[]): FileMeta[] {
  const seen = new Set<string>();
  const result: FileMeta[] = [];

  for (const m of matches) {
    if (!seen.has(m.relativePath)) {
      seen.add(m.relativePath);
      result.push({
        relativePath: m.relativePath,
        size: m.size,
        lineNumber: m.lineNumber,
        lineContent: m.lineContent,
        isDefinition: false, // fff-node doesn't expose is_definition yet
        matchRanges: m.matchRanges,
        contextAfter: m.contextAfter ?? [],
      });
    }
  }

  return result;
}

export type OutputMode = "content" | "files_with_matches" | "count" | "usage";

export function parseOutputMode(s: string | undefined): OutputMode {
  if (s === "files_with_matches" || s === "count" || s === "usage") return s;
  return "content";
}

export function formatGrepResult(
  matches: GrepMatch[],
  totalMatched: number,
  nextFileOffset: number,
  outputMode: OutputMode,
  maxResults: number,
  cursorStore: CursorStore,
): string {
  const items = matches.slice(0, maxResults);

  if (outputMode === "files_with_matches") {
    return formatFilesWithMatches(items, nextFileOffset, cursorStore);
  }

  if (outputMode === "count") {
    return formatCount(items, nextFileOffset, cursorStore);
  }

  // content / usage mode
  const lines: string[] = [];
  const uniqueFiles = new Set(items.map((m) => m.relativePath)).size;
  const isUsage = outputMode === "usage";

  const maxOutputChars =
    isUsage || uniqueFiles <= 3 ? 5000 : uniqueFiles <= 8 ? 3500 : 2500;

  // File overview
  const filePreview = collectFilePreview(items);
  const firstDef = filePreview.find((f) => f.isDefinition);
  const firstFile = filePreview[0];
  const contentSuggest = firstDef?.relativePath ?? firstFile?.relativePath;

  if (contentSuggest) {
    if (filePreview.length === 1) {
      lines.push(`→ Read ${contentSuggest} (only match)`);
    } else if (firstDef) {
      lines.push(`→ Read ${contentSuggest} [def]`);
    } else if (filePreview.length <= 3) {
      lines.push(`→ Read ${contentSuggest} (best match)`);
    }
  }

  if (totalMatched > items.length) {
    lines.push(`${items.length}/${totalMatched} matches shown`);
  }

  let charCount = 0;
  let shownCount = 0;
  let currentFile = "";

  for (const m of items) {
    const matchLines: string[] = [];

    if (m.relativePath !== currentFile) {
      currentFile = m.relativePath;
      matchLines.push(currentFile);
    }

    // Match line
    matchLines.push(
      ` ${m.lineNumber}: ${truncateLineForAi(m.lineContent, m.matchRanges, MAX_LINE_LEN)}`,
    );

    // Context after
    if (m.contextAfter && m.contextAfter.length > 0) {
      const expandLimit = shownCount === 0 ? MAX_DEF_EXPAND_FIRST : MAX_DEF_EXPAND;
      const startLine = m.lineNumber + 1;
      for (let i = 0; i < Math.min(m.contextAfter.length, expandLimit); i++) {
        const ctx = m.contextAfter[i];
        if (!ctx.trim()) break;
        matchLines.push(
          `  ${startLine + i}| ${truncateLineForAi(ctx, null, MAX_LINE_LEN)}`,
        );
      }
    }

    const chunk = matchLines.join("\n");
    if (charCount + chunk.length > maxOutputChars && shownCount > 0) break;

    charCount += chunk.length;
    lines.push(chunk);
    shownCount++;
  }

  if (nextFileOffset > 0) {
    const cursorId = cursorStore.store(nextFileOffset);
    lines.push(`\ncursor: ${cursorId}`);
  }

  return lines.join("\n");
}

function formatFilesWithMatches(
  items: GrepMatch[],
  nextFileOffset: number,
  cursorStore: CursorStore,
): string {
  const fileMap = collectFilePreview(items);
  const lines: string[] = [];

  const firstDef = fileMap.find((f) => f.isDefinition);
  const suggestPath = firstDef?.relativePath ?? fileMap[0]?.relativePath;

  if (suggestPath) {
    if (fileMap.length === 1) {
      lines.push(`→ Read ${suggestPath} (only match — no need to search further)`);
    } else if (firstDef && fileMap.length <= 5) {
      lines.push(`→ Read ${suggestPath} (definition found)`);
    } else if (firstDef) {
      lines.push(`→ Read ${suggestPath} (definition)`);
    } else if (fileMap.length <= 3) {
      lines.push(`→ Read ${suggestPath} (best match)`);
    } else {
      lines.push(`→ Read ${suggestPath}`);
    }
  }

  const isSmallSet = fileMap.length <= 5;

  for (let i = 0; i < fileMap.length; i++) {
    const fm = fileMap[i];
    const defTag = fm.isDefinition ? " [def]" : "";
    lines.push(`${fm.relativePath}${defTag}${sizeTag(fm.size)}`);

    if (fm.lineContent && (fm.isDefinition || i === 0 || isSmallSet)) {
      const ranges = fm.matchRanges.length > 0 ? fm.matchRanges : null;
      lines.push(
        `  ${fm.lineNumber}: ${truncateLineForAi(fm.lineContent, ranges, MAX_PREVIEW)}`,
      );
    }
  }

  if (nextFileOffset > 0) {
    const cursorId = cursorStore.store(nextFileOffset);
    lines.push(`\ncursor: ${cursorId}`);
  }

  return lines.join("\n");
}

function formatCount(
  items: GrepMatch[],
  nextFileOffset: number,
  cursorStore: CursorStore,
): string {
  const counts = new Map<string, number>();
  const order: string[] = [];

  for (const m of items) {
    const existing = counts.get(m.relativePath);
    if (existing === undefined) {
      order.push(m.relativePath);
      counts.set(m.relativePath, 1);
    } else {
      counts.set(m.relativePath, existing + 1);
    }
  }

  const lines = order.map((path) => `${path}: ${counts.get(path)}`);

  if (nextFileOffset > 0) {
    const cursorId = cursorStore.store(nextFileOffset);
    lines.push(`\ncursor: ${cursorId}`);
  }

  return lines.join("\n");
}
