import type { GrepCursor } from "@ff-labs/fff-bun";

let cursorCounter = 0;
const cursorStore = new Map<string, GrepCursor>();

export function storeCursor(cursor: GrepCursor): string {
  const id = String(++cursorCounter);
  cursorStore.set(id, cursor);
  // Evict old cursors (keep last 20)
  if (cursorStore.size > 20) {
    const oldest = cursorStore.keys().next().value;
    if (oldest) cursorStore.delete(oldest);
  }
  return id;
}

export function getCursor(id: string): GrepCursor | undefined {
  return cursorStore.get(id);
}
