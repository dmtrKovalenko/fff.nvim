/**
 * Binary download utilities for fff
 *
 * Downloads prebuilt binaries from GitHub releases based on commit hash.
 * The release tag corresponds to the short commit SHA (7 characters).
 */

import { existsSync, mkdirSync, writeFileSync, chmodSync, readFileSync } from "node:fs";
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
 * Get path to the hash file that tracks which version is installed
 */
function getHashFilePath(): string {
  return join(getBinDir(), ".hash");
}

/**
 * Check if the binary exists
 */
export function binaryExists(): boolean {
  return existsSync(getBinaryPath());
}

/**
 * Get the installed binary hash (if any)
 */
export function getInstalledHash(): string | null {
  const hashFile = getHashFilePath();
  if (existsSync(hashFile)) {
    return readFileSync(hashFile, "utf-8").trim();
  }
  return null;
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
 * Get the native binary hash from package.json
 * This is the commit hash that corresponds to the release tag
 */
async function getPackageHash(): Promise<string> {
  const packageDir = getPackageDir();
  const packageJsonPath = join(packageDir, "package.json");

  try {
    const pkg = await Bun.file(packageJsonPath).json();
    return pkg.nativeBinaryHash || "latest";
  } catch {
    return "latest";
  }
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
  const packageHash = hash || (await getPackageHash());
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

  // Save the hash for future reference
  writeFileSync(getHashFilePath(), resolvedHash);

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
  const currentHash = getInstalledHash();
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
