import type { GrepResult } from "@ff-labs/fff-bun";
import { storeCursor } from "./cursor";

/** Frecency score → single-token word. Empty for low-scoring files. */
export function frecencyWord(score: number): string {
  if (score >= 100) return "hot";
  if (score >= 50) return "warm";
  if (score >= 10) return "frequent";
  return "";
}

/** Git status → single-token word. Empty for clean files. */
export function gitWord(status: string): string {
  switch (status) {
    case "modified":
      return "modified";
    case "untracked":
      return "untracked";
    case "added":
    case "staged_new":
      return "staged";
    case "deleted":
      return "deleted";
    case "renamed":
      return "renamed";
    case "conflicted":
      return "conflicted";
    default:
      return "";
  }
}

/** Build " - hot git:modified" style suffix. Empty when nothing to report. */
export function fileSuffix(gitStatus: string, frecencyScore: number): string {
  const f = frecencyWord(frecencyScore);
  const g = gitWord(gitStatus);
  if (!f && !g) return "";
  const parts: string[] = [];
  if (f) parts.push(f);
  if (g) parts.push(`git:${g}`);
  return ` - ${parts.join(" ")}`;
}

export type OutputMode = "content" | "files_with_matches" | "count" | "usage";

/** Detect if a preview line looks like a definition (struct, fn, class, etc.) */
const DEF_RE =
  /^(?:pub(?:\([^)]*\))?\s+|export\s+(?:default\s+)?|async\s+|abstract\s+|unsafe\s+|static\s+|protected\s+|private\s+|public\s+)*(struct|fn|enum|trait|impl|class|interface|function|def|func|type|module|object)\b/;
function isDefinitionLine(line: string): boolean {
  return DEF_RE.test(line.trimStart());
}

/** Detect import/use lines — lower value than definitions or usages */
const IMPORT_RE = /^\s*(?:import\s|from\s['"]|use\s|require\s*\(|#\s*include\s)/;
function isImportLine(line: string): boolean {
  return IMPORT_RE.test(line);
}

const LARGE_FILE_BYTES = 20_000;
/** Tag for large files — nudges model to use offset/limit when reading. */
function sizeTag(bytes: number): string {
  if (bytes < LARGE_FILE_BYTES) return "";
  const kb = Math.round(bytes / 1024);
  return ` (${kb}KB - use offset to read relevant section)`;
}

/**
 * Truncate a line centered on the match region.
 * Shows 1/3 context before match, match itself, then remaining budget after.
 * Falls back to start-truncation when no match ranges are available.
 */
function truncCentered(
  line: string,
  matchRanges: [number, number][] | undefined,
  maxLen: number,
): string {
  if (line.length <= maxLen) return line;

  // Use first match range to center the window
  if (matchRanges && matchRanges.length > 0) {
    const [matchStart, matchEnd] = matchRanges[0];
    const matchLen = matchEnd - matchStart;

    // Allocate 1/3 of remaining budget before match, 2/3 after
    const budget = maxLen - matchLen;
    const before = Math.max(0, Math.floor(budget / 3));
    const after = budget - before;

    const winStart = Math.max(0, matchStart - before);
    const winEnd = Math.min(line.length, matchEnd + after);

    let result = line.slice(winStart, winEnd);
    if (winStart > 0) result = "…" + result;
    if (winEnd < line.length) result = result + "…";
    return result;
  }

  // No match ranges — truncate from start
  return line.slice(0, maxLen) + "…";
}

const MAX_PREVIEW = 120;
const MAX_LINE_LEN = 180;
/** Max context lines to show when auto-expanding the first definition */
const MAX_DEF_EXPAND_FIRST = 8;
/** Max context lines for subsequent definitions */
const MAX_DEF_EXPAND = 5;
/** Max context lines for non-definition first match in small result sets */
const MAX_FIRST_MATCH_EXPAND = 8;

interface FileMeta {
  gitStatus: string;
  frecencyScore: number;
  lineNumber: number;
  lineContent: string;
  matchRanges: [number, number][];
  size: number;
  contextAfter: string[];
}

export function formatGrepResults(
  result: GrepResult,
  outputMode: OutputMode,
  maxResults: number,
  regexFallbackError?: string,
  showContext?: boolean,
  autoExpandDefs?: boolean,
): string {
  const { items: allItems, totalMatched, nextCursor } = result;

  const items = allItems.slice(0, maxResults);

  if (outputMode === "files_with_matches") {
    // Group by file, keep first match's data including matchRanges + context
    const fileMap = new Map<string, FileMeta>();
    for (const match of items) {
      if (!fileMap.has(match.relativePath)) {
        fileMap.set(match.relativePath, {
          gitStatus: match.gitStatus,
          frecencyScore: match.totalFrecencyScore,
          lineNumber: match.lineNumber,
          lineContent: match.lineContent,
          matchRanges: match.matchRanges ?? [],
          size: match.size,
          contextAfter: match.contextAfter ?? [],
        });
      }
    }

    const lines: string[] = [];
    const fileCount = fileMap.size;

    // Find best Read target: prefer [def] file, fallback to first file
    let firstDefFile = "";
    let firstFile = "";
    for (const [path, meta] of fileMap) {
      if (!firstFile) firstFile = path;
      if (!firstDefFile && meta.lineContent && isDefinitionLine(meta.lineContent)) {
        firstDefFile = path;
      }
    }
    const suggestPath = firstDefFile || firstFile;

    // Confidence-based suggestion — stronger signal = more assertive
    if (suggestPath) {
      if (fileCount === 1) {
        lines.push(`→ Read ${suggestPath} (only match — no need to search further)`);
      } else if (firstDefFile && fileCount <= 5) {
        lines.push(`→ Read ${suggestPath} (definition found)`);
      } else if (firstDefFile) {
        lines.push(`→ Read ${suggestPath} (definition)`);
      } else if (fileCount <= 3) {
        lines.push(`→ Read ${suggestPath} (best match)`);
      } else {
        lines.push(`→ Read ${suggestPath}`);
      }
    }

    let fileIdx = 0;
    let defExpandedCount = 0;
    // Small result sets get aggressive expansion for ALL matches
    const isSmallSet = fileCount <= 5;
    for (const [path, meta] of fileMap) {
      const isDef = meta.lineContent ? isDefinitionLine(meta.lineContent) : false;
      const defTag = isDef ? " [def]" : "";
      lines.push(`${path}${defTag}${sizeTag(meta.size)}`);
      // Show preview for [def] files, first file, and all files in small sets
      if (meta.lineContent && (isDef || fileIdx === 0 || isSmallSet)) {
        lines.push(
          `  ${meta.lineNumber}: ${truncCentered(meta.lineContent, meta.matchRanges, MAX_PREVIEW)}`,
        );
        // Auto-expand body context — definitions get priority, but small sets expand everything
        if (autoExpandDefs && meta.contextAfter.length > 0) {
          let expandLimit: number;
          if (isDef) {
            expandLimit = defExpandedCount === 0 ? MAX_DEF_EXPAND_FIRST : MAX_DEF_EXPAND;
            defExpandedCount++;
          } else if (isSmallSet && fileIdx === 0) {
            expandLimit = MAX_FIRST_MATCH_EXPAND;
          } else if (isSmallSet) {
            expandLimit = MAX_DEF_EXPAND;
          } else {
            expandLimit = 0;
          }
          if (expandLimit > 0) {
            const startLine = meta.lineNumber + 1;
            for (let i = 0; i < Math.min(meta.contextAfter.length, expandLimit); i++) {
              const ctx = meta.contextAfter[i];
              if (ctx.trim() === "") break;
              lines.push(`  ${startLine + i}| ${truncCentered(ctx, undefined, MAX_PREVIEW)}`);
            }
          }
        }
      }
      fileIdx++;
    }

    if (nextCursor) {
      lines.push(`\ncursor: ${storeCursor(nextCursor)}`);
    }
    return lines.join("\n");
  }

  if (outputMode === "count") {
    // Group by file, count matches per file
    const countMap = new Map<string, number>();
    for (const match of items) {
      countMap.set(match.relativePath, (countMap.get(match.relativePath) ?? 0) + 1);
    }
    const totalCount = items.length;
    const lines: string[] = [];
    lines.push(`${totalCount} matches in ${countMap.size} files`);
    for (const [path, count] of countMap) {
      lines.push(`${path}: ${count}`);
    }
    if (nextCursor) {
      lines.push(`\ncursor: ${storeCursor(nextCursor)}`);
    }
    return lines.join("\n");
  }

  // "content" and "usage" modes
  // Adaptive budget: small result sets get more detail to avoid follow-up Reads
  const lines: string[] = [];
  const uniqueFiles = new Set(items.map((m) => m.relativePath)).size;
  const MAX_OUTPUT_CHARS =
    outputMode === "usage" ? 5000 : uniqueFiles <= 3 ? 5000 : uniqueFiles <= 8 ? 3500 : 2500;

  if (regexFallbackError) {
    lines.push(`! regex failed: ${regexFallbackError}, using literal match`);
  }

  // File overview with first-match previews (always shown, outside budget)
  const filePreview = new Map<string, FileMeta>();
  for (const match of items) {
    if (!filePreview.has(match.relativePath)) {
      filePreview.set(match.relativePath, {
        gitStatus: match.gitStatus,
        frecencyScore: match.totalFrecencyScore,
        lineNumber: match.lineNumber,
        lineContent: match.lineContent,
        matchRanges: match.matchRanges ?? [],
        size: match.size,
        contextAfter: match.contextAfter ?? [],
      });
    }
  }

  // Find best Read target: prefer [def] file, fallback to first
  let contentDefFile = "";
  let contentFirstFile = "";
  for (const [path, meta] of filePreview) {
    if (!contentFirstFile) contentFirstFile = path;
    if (!contentDefFile && meta.lineContent && isDefinitionLine(meta.lineContent)) {
      contentDefFile = path;
    }
  }
  const contentSuggest = contentDefFile || contentFirstFile;
  if (contentSuggest) {
    const fileCount = filePreview.size;
    if (fileCount === 1) {
      lines.push(`→ Read ${contentSuggest} (only match)`);
    } else if (contentDefFile) {
      lines.push(`→ Read ${contentSuggest} [def]`);
    } else if (fileCount <= 3) {
      lines.push(`→ Read ${contentSuggest} (best match)`);
    }
  }

  lines.push(`${filePreview.size} files`);

  if (totalMatched > items.length) {
    lines.push(`${items.length}/${totalMatched} matches shown`);
  }

  // Track which files already had a definition expanded (limit 1 expansion per file)
  const defExpandedFiles = new Set<string>();

  // Detailed content (subject to budget)
  let charCount = 0;
  let shownCount = 0;
  let currentFile = "";

  // Reorder: definitions first, then usages, then imports (when auto-expanding)
  const sortedItems = autoExpandDefs
    ? [...items].sort((a, b) => {
        const aDef = isDefinitionLine(a.lineContent) ? 0 : isImportLine(a.lineContent) ? 2 : 1;
        const bDef = isDefinitionLine(b.lineContent) ? 0 : isImportLine(b.lineContent) ? 2 : 1;
        return aDef - bDef;
      })
    : items;

  for (const match of sortedItems) {
    const matchLines: string[] = [];

    if (match.relativePath !== currentFile) {
      currentFile = match.relativePath;
      matchLines.push(currentFile);
    }

    // Skip import-only lines when we already have definitions (they add noise, not signal)
    if (autoExpandDefs && isImportLine(match.lineContent) && defExpandedFiles.size > 0) {
      continue;
    }

    // Context before (only when explicitly requested)
    if (showContext && match.contextBefore && match.contextBefore.length > 0) {
      const startLine = match.lineNumber - match.contextBefore.length;
      for (let i = 0; i < match.contextBefore.length; i++) {
        matchLines.push(
          ` ${startLine + i}-${truncCentered(match.contextBefore[i], undefined, MAX_LINE_LEN)}`,
        );
      }
    }

    // Match line — centered on the match
    matchLines.push(
      ` ${match.lineNumber}: ${truncCentered(match.lineContent, match.matchRanges, MAX_LINE_LEN)}`,
    );

    // Context after (only when explicitly requested via context parameter)
    if (showContext && match.contextAfter && match.contextAfter.length > 0) {
      const startLine = match.lineNumber + 1;
      for (let i = 0; i < match.contextAfter.length; i++) {
        matchLines.push(
          ` ${startLine + i}-${truncCentered(match.contextAfter[i], undefined, MAX_LINE_LEN)}`,
        );
      }
      matchLines.push("--");
    }

    // Auto-expand definitions with body context (eliminates follow-up Read calls)
    if (
      autoExpandDefs &&
      !showContext &&
      isDefinitionLine(match.lineContent) &&
      match.contextAfter?.length &&
      !defExpandedFiles.has(match.relativePath)
    ) {
      const expandLimit = defExpandedFiles.size === 0 ? MAX_DEF_EXPAND_FIRST : MAX_DEF_EXPAND;
      defExpandedFiles.add(match.relativePath);
      const startLine = match.lineNumber + 1;
      for (let i = 0; i < Math.min(match.contextAfter.length, expandLimit); i++) {
        const ctx = match.contextAfter[i];
        if (ctx.trim() === "") break;
        matchLines.push(`  ${startLine + i}| ${truncCentered(ctx, undefined, MAX_LINE_LEN)}`);
      }
    }

    const chunk = matchLines.join("\n");
    if (charCount + chunk.length > MAX_OUTPUT_CHARS && shownCount > 0) {
      break;
    }

    lines.push(chunk);
    charCount += chunk.length;
    shownCount++;
  }

  if (nextCursor) {
    lines.push(`\ncursor: ${storeCursor(nextCursor)}`);
  }

  return lines.join("\n");
}
