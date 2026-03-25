import type { FileFinder } from "@ff-labs/fff-node";
import type { CursorStore } from "../cursor-store.js";
import { formatGrepResult, parseOutputMode } from "../output-formatter.js";

export function executeMultiGrep(
  finder: FileFinder,
  patterns: string[],
  constraints: string | undefined,
  maxResults: number,
  cursorId: string | undefined,
  outputModeStr: string | undefined,
  cursorStore: CursorStore,
): string {
  const outputMode = parseOutputMode(outputModeStr);
  const fileOffset = cursorId ? (cursorStore.get(cursorId) ?? 0) : 0;

  const isUsage = outputMode === "usage";
  const matchesPerFile = outputMode === "files_with_matches" ? 1 : isUsage ? 8 : 10;
  const afterContext = isUsage ? 1 : 8;

  const result = finder.multiGrep({
    patterns,
    constraints,
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

  // Fallback: on 0 multi-pattern results, try individual patterns with plain grep
  if (items.length === 0 && fileOffset === 0) {
    for (const pat of patterns) {
      const fullQuery = constraints ? `${constraints} ${pat}` : pat;
      const fallback = finder.grep(fullQuery, {
        mode: "plain",
        maxMatchesPerFile: matchesPerFile,
        timeBudgetMs: 3000,
      });

      if (fallback.ok && fallback.value.items.length > 0) {
        const fbNext = fallback.value.nextCursor?._offset ?? 0;
        const text = formatGrepResult(
          fallback.value.items,
          fallback.value.totalMatched,
          fbNext,
          outputMode,
          maxResults,
          cursorStore,
        );
        return `0 multi-pattern matches. Plain grep fallback for "${pat}":\n${text}`;
      }
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
