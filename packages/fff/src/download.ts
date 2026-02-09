/**
 * Binary download utilities for fff
 *
 * Downloads prebuilt binaries from GitHub releases, similar to
 * how the Neovim Lua version handles binary distribution.
 */

import { existsSync, mkdirSync, writeFileSync, chmodSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { getTriple, getLibExtension, getLibFilename } from "./platform";

const GITHUB_REPO = "dmtrKovalenko/fff.nvim";

/**
 * Get the current file's directory
 */
function getCurrentDir(): string {
  // Handle both file:// URLs and regular paths
  const url = import.meta.url;
  if (url.startsWith("file://")) {
    return dirname(fileURLToPath(url));
  }
  return dirname(url);
}

/**
 * Get the package root directory
 */
function getPackageDir(): string {
  const currentDir = getCurrentDir();
  // src/download.ts -> go up one level to package root
  return dirname(currentDir);
}

/**
 * Get the directory where binaries are stored
 */
export function getBinDir(): string {
  // When installed as a package, binaries go in the package's bin directory
  return join(getPackageDir(), "bin");
}

/**
 * Get the full path to the native library
 */
export function getBinaryPath(): string {
  const binDir = getBinDir();
  return join(binDir, getLibFilename());
}

/**
 * Check if the binary exists
 */
export function binaryExists(): boolean {
  return existsSync(getBinaryPath());
}

/**
 * Get the development binary path (for local development)
 */
export function getDevBinaryPath(): string | null {
  // Check for local cargo build
  const packageDir = getPackageDir();

  // Try workspace root target directory (packages/fff -> fff.nvim root)
  const workspaceRoot = join(packageDir, "..", "..");

  const possiblePaths = [
    join(workspaceRoot, "target", "release", getLibFilename()),
    join(workspaceRoot, "target", "debug", getLibFilename()),
  ];

  for (const path of possiblePaths) {
    if (existsSync(path)) {
      return path;
    }
  }

  return null;
}

/**
 * Find the binary, checking both installed and dev paths
 */
export function findBinary(): string | null {
  // First check installed location
  const installedPath = getBinaryPath();
  if (existsSync(installedPath)) {
    return installedPath;
  }

  // Then check dev location
  return getDevBinaryPath();
}

/**
 * Get the current package version from package.json
 */
async function getPackageVersion(): Promise<string> {
  const packageDir = getPackageDir();
  const packageJsonPath = join(packageDir, "package.json");

  try {
    const pkg = await Bun.file(packageJsonPath).json();
    // Use nativeVersion if specified, otherwise use package version
    return pkg.nativeVersion || pkg.version;
  } catch {
    return "latest";
  }
}

/**
 * Download the binary from GitHub releases
 */
export async function downloadBinary(version?: string): Promise<void> {
  const targetVersion = version || (await getPackageVersion());
  const triple = getTriple();
  const ext = getLibExtension();

  // Binary name format: c-lib-{triple}.{ext}
  const binaryName = `c-lib-${triple}.${ext}`;
  const url = `https://github.com/${GITHUB_REPO}/releases/download/${targetVersion}/${binaryName}`;

  console.log(`fff: Downloading native library for ${triple}...`);
  console.log(`fff: URL: ${url}`);

  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(
      `Failed to download binary: ${response.status} ${response.statusText}\n` +
        `URL: ${url}\n` +
        `Make sure version "${targetVersion}" exists and has binaries for ${triple}`,
    );
  }

  const binDir = getBinDir();
  if (!existsSync(binDir)) {
    mkdirSync(binDir, { recursive: true });
  }

  const binaryPath = getBinaryPath();
  const buffer = await response.arrayBuffer();
  writeFileSync(binaryPath, Buffer.from(buffer));

  // Make executable on Unix
  if (process.platform !== "win32") {
    chmodSync(binaryPath, 0o755);
  }

  console.log(`fff: Binary downloaded to ${binaryPath}`);
}

/**
 * Ensure the binary exists, downloading if necessary
 */
export async function ensureBinary(): Promise<string> {
  // First check if binary already exists
  const existingPath = findBinary();
  if (existingPath) {
    return existingPath;
  }

  // Download binary
  await downloadBinary();
  return getBinaryPath();
}

/**
 * Download binary, with fallback to cargo build instructions
 */
export async function downloadOrBuild(): Promise<void> {
  try {
    await downloadBinary();
  } catch (error) {
    console.error(`fff: Failed to download binary: ${error}`);
    console.error(`fff: You can build from source instead:`);
    console.error(`  cargo build --release -p fff-c`);
    throw error;
  }
}
