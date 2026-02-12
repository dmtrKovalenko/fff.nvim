/**
 * Binary download utilities for fff
 *
 * Downloads prebuilt binaries from GitHub releases based on commit hash.
 * The release tag corresponds to the short commit SHA (7 characters).
 */

import { existsSync, mkdirSync, writeFileSync, chmodSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import { getTriple, getLibExtension, getLibFilename } from "./platform";

const GITHUB_REPO = "dmtrKovalenko/fff.nvim";
const GITHUB_API = "https://api.github.com";

/**
 * Get the current file's directory
 */
function getCurrentDir(): string {
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
  return dirname(currentDir);
}

/**
 * Get the path to package.json
 */
function getPackageJsonPath(): string {
  return join(getPackageDir(), "package.json");
}

/**
 * Get the directory where binaries are stored
 */
export function getBinDir(): string {
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
 * Read package.json
 */
async function readPackageJson(): Promise<Record<string, unknown>> {
  try {
    return await Bun.file(getPackageJsonPath()).json();
  } catch {
    return {};
  }
}

/**
 * Write package.json
 */
async function writePackageJson(pkg: Record<string, unknown>): Promise<void> {
  const content = JSON.stringify(pkg, null, 2) + "\n";
  writeFileSync(getPackageJsonPath(), content);
}

/**
 * Get the installed binary hash from package.json
 */
export async function getInstalledHash(): Promise<string | null> {
  const pkg = await readPackageJson();
  return (pkg.nativeBinaryHash as string) || null;
}

/**
 * Update the installed hash in package.json
 */
async function setInstalledHash(hash: string): Promise<void> {
  const pkg = await readPackageJson();
  pkg.nativeBinaryHash = hash;
  await writePackageJson(pkg);
}

/**
 * Get the development binary path (for local development)
 */
export function getDevBinaryPath(): string | null {
  const packageDir = getPackageDir();
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
  const installedPath = getBinaryPath();
  if (existsSync(installedPath)) {
    return installedPath;
  }
  return getDevBinaryPath();
}

/**
 * Fetch the latest release tag from GitHub
 */
async function fetchLatestReleaseTag(): Promise<string> {
  const url = `${GITHUB_API}/repos/${GITHUB_REPO}/releases/latest`;
  
  const response = await fetch(url, {
    headers: {
      "Accept": "application/vnd.github.v3+json",
      "User-Agent": "fff-bun-client",
    },
  });

  if (!response.ok) {
    // If no "latest" release, try getting the most recent prerelease
    const allReleasesUrl = `${GITHUB_API}/repos/${GITHUB_REPO}/releases`;
    const allResponse = await fetch(allReleasesUrl, {
      headers: {
        "Accept": "application/vnd.github.v3+json",
        "User-Agent": "fff-bun-client",
      },
    });

    if (!allResponse.ok) {
      throw new Error(`Failed to fetch releases: ${allResponse.status}`);
    }

    const releases = await allResponse.json() as Array<{ tag_name: string }>;
    if (releases.length === 0) {
      throw new Error("No releases found");
    }

    return releases[0].tag_name;
  }

  const release = await response.json() as { tag_name: string };
  return release.tag_name;
}

/**
 * Resolve the hash to use for downloading
 * If "latest", fetches the latest release tag from GitHub
 */
async function resolveHash(hash: string): Promise<string> {
  if (hash === "latest") {
    console.log("fff: Fetching latest release tag...");
    return await fetchLatestReleaseTag();
  }
  return hash;
}

/**
 * Download and verify checksum for a binary
 */
async function downloadWithChecksum(
  binaryUrl: string,
  checksumUrl: string,
): Promise<Buffer> {
  // Download binary
  const binaryResponse = await fetch(binaryUrl);
  if (!binaryResponse.ok) {
    throw new Error(
      `Failed to download binary: ${binaryResponse.status} ${binaryResponse.statusText}\nURL: ${binaryUrl}`,
    );
  }

  const binaryBuffer = Buffer.from(await binaryResponse.arrayBuffer());

  // Try to download and verify checksum
  try {
    const checksumResponse = await fetch(checksumUrl);
    if (checksumResponse.ok) {
      const checksumText = await checksumResponse.text();
      // Format: "hash  filename" or just "hash"
      const expectedHash = checksumText.trim().split(/\s+/)[0];
      
      const actualHash = createHash("sha256").update(binaryBuffer).digest("hex");
      
      if (actualHash !== expectedHash) {
        throw new Error(
          `Checksum mismatch!\nExpected: ${expectedHash}\nActual: ${actualHash}`,
        );
      }
      console.log("fff: Checksum verified ✓");
    }
  } catch (error) {
    if (error instanceof Error && error.message.includes("Checksum mismatch")) {
      throw error;
    }
    // Checksum file not found, continue without verification
    console.log("fff: Checksum file not available, skipping verification");
  }

  return binaryBuffer;
}

/**
 * Download the binary from GitHub releases
 * @param hash - The commit hash (release tag) to download, or "latest"
 */
export async function downloadBinary(hash?: string): Promise<string> {
  const currentHash = await getInstalledHash();
  const packageHash = hash || currentHash || "latest";
  const resolvedHash = await resolveHash(packageHash);
  
  const triple = getTriple();
  const ext = getLibExtension();

  // Binary name format: c-lib-{triple}.{ext}
  const binaryName = `c-lib-${triple}.${ext}`;
  const baseUrl = `https://github.com/${GITHUB_REPO}/releases/download/${resolvedHash}`;
  const binaryUrl = `${baseUrl}/${binaryName}`;
  const checksumUrl = `${baseUrl}/${binaryName}.sha256`;

  console.log(`fff: Downloading native library for ${triple}...`);
  console.log(`fff: Release: ${resolvedHash}`);
  console.log(`fff: URL: ${binaryUrl}`);

  const binaryBuffer = await downloadWithChecksum(binaryUrl, checksumUrl);

  const binDir = getBinDir();
  if (!existsSync(binDir)) {
    mkdirSync(binDir, { recursive: true });
  }

  const binaryPath = getBinaryPath();
  writeFileSync(binaryPath, binaryBuffer);

  // Save the hash to package.json
  await setInstalledHash(resolvedHash);

  // Make executable on Unix
  if (process.platform !== "win32") {
    chmodSync(binaryPath, 0o755);
  }

  console.log(`fff: Binary downloaded to ${binaryPath}`);
  return resolvedHash;
}

/**
 * Check if an update is available
 */
export async function checkForUpdate(): Promise<{
  currentHash: string | null;
  latestHash: string;
  updateAvailable: boolean;
}> {
  const currentHash = await getInstalledHash();
  const latestHash = await fetchLatestReleaseTag();
  
  return {
    currentHash,
    latestHash,
    updateAvailable: currentHash !== latestHash,
  };
}

/**
 * Ensure the binary exists, downloading if necessary
 */
export async function ensureBinary(): Promise<string> {
  const existingPath = findBinary();
  if (existingPath) {
    return existingPath;
  }

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
