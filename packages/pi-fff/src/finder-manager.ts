import { mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import type { Result } from "@ff-labs/fff-node";
import { FileFinder } from "@ff-labs/fff-node";

const DB_DIR = join(homedir(), ".local", "share", "fff");
const FRECENCY_DB = join(DB_DIR, "pi-frecency.mdb");
const HISTORY_DB = join(DB_DIR, "pi-history.mdb");

let finder: FileFinder | null = null;
let currentBasePath = "";

function ensureDbDir(): void {
  mkdirSync(DB_DIR, { recursive: true });
}

export function getFinder(cwd: string): Result<FileFinder> {
  if (finder && !finder.isDestroyed && currentBasePath === cwd) {
    return { ok: true, value: finder };
  }

  if (finder && !finder.isDestroyed) {
    finder.destroy();
    finder = null;
  }

  ensureDbDir();

  const result = FileFinder.create({
    basePath: cwd,
    aiMode: true,
    frecencyDbPath: FRECENCY_DB,
    historyDbPath: HISTORY_DB,
    warmupMmapCache: true,
  });

  if (!result.ok) {
    return result;
  }

  finder = result.value;
  currentBasePath = cwd;
  return { ok: true, value: finder };
}

export function reindex(newPath: string): Result<void> {
  if (!finder || finder.isDestroyed) {
    const result = getFinder(newPath);
    if (!result.ok) return result;
    return { ok: true, value: undefined };
  }

  currentBasePath = newPath;
  return finder.reindex(newPath);
}

export function destroy(): void {
  if (finder && !finder.isDestroyed) {
    finder.destroy();
    finder = null;
    currentBasePath = "";
  }
}
