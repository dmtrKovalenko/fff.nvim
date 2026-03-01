let cursorCounter = 0;
const cursorStore = new Map();
export function storeCursor(cursor) {
    const id = String(++cursorCounter);
    cursorStore.set(id, cursor);
    // Evict old cursors (keep last 20)
    if (cursorStore.size > 20) {
        const oldest = cursorStore.keys().next().value;
        if (oldest)
            cursorStore.delete(oldest);
    }
    return id;
}
export function getCursor(id) {
    return cursorStore.get(id);
}
