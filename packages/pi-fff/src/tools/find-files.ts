import type { FileFinder } from "@ff-labs/fff-node";
import type { CursorStore } from "../cursor-store.js";
import { formatFindFilesResult } from "../output-formatter.js";

export function executeFindFiles(
  finder: FileFinder,
  query: string,
  maxResults: number,
  cursorId: string | undefined,
  cursorStore: CursorStore,
): string {
  const pageOffset = cursorId ? (cursorStore.get(cursorId) ?? 0) : 0;
  const pageSize = maxResults;

  const result = finder.fileSearch(query, {
    pageSize,
    pageIndex: pageSize > 0 && pageOffset > 0 ? Math.floor(pageOffset / pageSize) : 0,
    comboBoostMultiplier: 100,
    minComboCount: 3,
  });

  if (!result.ok) {
    return `Error: ${result.error}`;
  }

  const { items, scores, totalMatched, totalFiles } = result.value;

  // Auto-retry: if 3+ words return 0 results, retry with first 2 words
  if (items.length === 0 && pageOffset === 0) {
    const words = query.split(/\s+/);
    if (words.length >= 3) {
      const shorter = words.slice(0, 2).join(" ");
      const retry = finder.fileSearch(shorter, {
        pageSize,
        comboBoostMultiplier: 100,
        minComboCount: 3,
      });

      if (retry.ok && retry.value.items.length > 0) {
        return formatFindFilesResult(
          retry.value.items,
          retry.value.scores,
          retry.value.totalMatched,
          retry.value.totalFiles,
          0,
          cursorStore,
        );
      }
    }
  }

  return formatFindFilesResult(
    items,
    scores,
    totalMatched,
    totalFiles,
    pageOffset,
    cursorStore,
  );
}
