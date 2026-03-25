import type { FileFinder, GrepMode } from "@ff-labs/fff-node";
import type { CursorStore } from "../cursor-store.js";
import {
  formatGrepResult,
  type OutputMode,
  parseOutputMode,
} from "../output-formatter.js";

const REGEX_META = /[.*+?^${}()|[\]\\]/;

function hasRegexMeta(s: string): boolean {
  return REGEX_META.test(s);
}

function performGrep(
  finder: FileFinder,
  query: string,
  mode: GrepMode,
  maxResults: number,
  fileOffset: number,
  outputMode: OutputMode,
  cursorStore: CursorStore,
): string {
  const isUsage = outputMode === "usage";
  const matchesPerFile = outputMode === "files_with_matches" ? 1 : isUsage ? 8 : 10;
  const afterContext = isUsage ? 1 : 8; // 8 for auto-expand defs

  const result = finder.grep(query, {
    mode,
    maxMatchesPerFile: matchesPerFile,
    afterContext,
    beforeContext: isUsage ? 1 : 0,
    cursor:
      fileOffset > 0
        ? ({ __brand: "GrepCursor" as const, _offset: fileOffset } as any)
        : null,
  });

  if (!result.ok) {
    return `Error: ${result.error}`;
  }

  const { items, totalMatched, nextCursor } = result.value;
  const nextFileOffset = nextCursor?._offset ?? 0;

  // Auto-retry on 0 results (first page only)
  if (items.length === 0 && fileOffset === 0) {
    // Try broadening multi-word queries by dropping first non-constraint word
    const parts = query.split(/\s+/);
    if (parts.length >= 2) {
      const first = parts[0];
      const isConstraint =
        first.startsWith("!") || first.startsWith("*") || first.endsWith("/");

      if (!isConstraint) {
        const restQuery = parts.slice(1).join(" ");
        const retryMode: GrepMode = hasRegexMeta(restQuery) ? "regex" : mode;

        const retry = finder.grep(restQuery, {
          mode: retryMode,
          maxMatchesPerFile: matchesPerFile,
          afterContext,
        });

        if (retry.ok && retry.value.items.length > 0 && retry.value.items.length <= 10) {
          const retryNext = retry.value.nextCursor?._offset ?? 0;
          const text = formatGrepResult(
            retry.value.items,
            retry.value.totalMatched,
            retryNext,
            outputMode,
            maxResults,
            cursorStore,
          );
          return `0 matches for '${query}'. Auto-broadened to '${restQuery}':\n${text}`;
        }
      }
    }

    // Fuzzy fallback for typo tolerance
    const fuzzyQuery = query.replace(/[:_-]/g, "").toLowerCase();
    const fuzzyResult = finder.grep(fuzzyQuery, {
      mode: "fuzzy",
      maxMatchesPerFile: 3,
    });

    if (fuzzyResult.ok && fuzzyResult.value.items.length > 0) {
      const lines = [`0 exact matches. ${fuzzyResult.value.items.length} approximate:`];
      let currentFile = "";
      for (const m of fuzzyResult.value.items.slice(0, 5)) {
        if (m.relativePath !== currentFile) {
          currentFile = m.relativePath;
          lines.push(currentFile);
        }
        lines.push(` ${m.lineNumber}: ${m.lineContent.trim().slice(0, 180)}`);
      }
      return lines.join("\n");
    }

    return "0 matches.";
  }

  if (items.length === 0) {
    return "0 matches.";
  }

  return formatGrepResult(
    items,
    totalMatched,
    nextFileOffset,
    outputMode,
    maxResults,
    cursorStore,
  );
}

export function executeGrep(
  finder: FileFinder,
  query: string,
  maxResults: number,
  cursorId: string | undefined,
  outputModeStr: string | undefined,
  cursorStore: CursorStore,
): string {
  const outputMode = parseOutputMode(outputModeStr);
  const fileOffset = cursorId ? (cursorStore.get(cursorId) ?? 0) : 0;

  // Auto-detect mode: regex if metacharacters present, else plain text
  const mode: GrepMode = hasRegexMeta(query) ? "regex" : "plain";

  return performGrep(
    finder,
    query,
    mode,
    maxResults,
    fileOffset,
    outputMode,
    cursorStore,
  );
}
