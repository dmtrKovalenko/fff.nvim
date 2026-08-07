import os from "node:os";
import path from "node:path";

// Resolved once per process: os.homedir() hits the env/passwd on every call.
export const HOME_DIR = path.resolve(os.homedir());

export function isHomeDir(dir: string): boolean {
  return path.resolve(dir) === HOME_DIR;
}
