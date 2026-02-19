/**
 * Binary resolution utilities for fff
 *
 * Resolves the native binary from:
 * 1. Platform-specific npm package (e.g. @ff-labs/fff-bun-darwin-arm64) - primary
 * 2. Local bin/ directory (legacy or manual download)
 * 3. Local dev build (target/release or target/debug)
 * 4. GitHub releases (fallback, requires network)
 */

import { existsSync, mkdirSync, writeFileSync, chmodSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import {
  getTriple,
  getLibExtension,
  getLibFilename,
  getNpmPackageName,
} from "./platform";

const GITHUB_REPO = "dmtrKovalenko/fff.nvim";

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
 * Get the directory where binaries are stored (legacy/fallback)
 */
export function getBinDir(): string {
  return join(getPackageDir(), "bin");
}

/**
 * Get the full path to the native library in bin/ (legacy/fallback)
 */
export function getBinaryPath(): string {
  const binDir = getBinDir();
  return join(binDir, getLibFilename());
}

/**
 * Check if the binary exists in any known location
 */
export function binaryExists(): boolean {
  return findBinary() !== null;
}

/**
 * Try to resolve the binary from the platform-specific npm package.
 *
 * When users install @ff-labs/bun, npm/bun automatically installs the matching
 * optionalDependency (e.g. @ff-labs/fff-bun-darwin-arm64). We resolve the binary
 * path by requiring that package's package.json and looking for the binary
 * in the same directory.
 */
function resolveFromNpmPackage(): string | null {
  const packageName = getNpmPackageName();

  try {
    // Use createRequire to resolve the platform package's location
    const require = createRequire(join(getPackageDir(), "package.json"));
    const packageJsonPath = require.resolve(`${packageName}/package.json`);
    const packageDir = dirname(packageJsonPath);
    const binaryPath = join(packageDir, getLibFilename());

    if (existsSync(binaryPath)) {
      return binaryPath;
    }
  } catch {
    // Package not installed - this is expected on unsupported platforms
    // or when installed without optional dependencies
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

function isDevWorkspace(): boolean {
  const packageDir = getPackageDir();
  const workspaceRoot = join(packageDir, "..", "..");
  return existsSync(join(workspaceRoot, "Cargo.toml"));
}

export function findBinary(): string | null {
  if (isDevWorkspace()) {
    // 1. Local bin/ directory (populated by `make prepare-bun`)
    const installedPath = getBinaryPath();
    if (existsSync(installedPath)) return installedPath;

    // 2. Local dev build (target/release or target/debug)
    const devPath = getDevBinaryPath();
    if (devPath) return devPath;

    // 3. Fallback to npm package
    const npmPath = resolveFromNpmPackage();
    if (npmPath) return npmPath;

    return null;
  }

  // Production: npm package first
  // 1. Try platform-specific npm package first
  const npmPath = resolveFromNpmPackage();
  if (npmPath) return npmPath;

  // 2. Try local bin/ directory (legacy or manual download)
  const installedPath = getBinaryPath();
  if (existsSync(installedPath)) return installedPath;

  // 3. Try local dev build
  return getDevBinaryPath();
}

/**
 * Download the binary from GitHub releases as a fallback.
 * This is only used when the platform npm package is not available.
 *
 * @param tag - The release tag to download (e.g. commit hash), or "latest"
 */
export async function downloadBinary(tag?: string): Promise<string> {
  const resolvedTag = tag || "latest";
  const triple = getTriple();
  const ext = getLibExtension();

  // Resolve "latest" tag via GitHub API
  let releaseTag = resolvedTag;
  if (releaseTag === "latest") {
    console.log("fff: Fetching latest release tag from GitHub...");
    releaseTag = await fetchLatestReleaseTag();
  }

  const binaryName = `c-lib-${triple}.${ext}`;
  const baseUrl = `https://github.com/${GITHUB_REPO}/releases/download/${releaseTag}`;
  const binaryUrl = `${baseUrl}/${binaryName}`;

  console.log(`fff: Downloading native library for ${triple}...`);
  console.log(`fff: Release: ${releaseTag}`);

  const binaryResponse = await fetch(binaryUrl);
  if (!binaryResponse.ok) {
    throw new Error(
      `Failed to download binary: ${binaryResponse.status} ${binaryResponse.statusText}\nURL: ${binaryUrl}`,
    );
  }

  const binaryBuffer = Buffer.from(await binaryResponse.arrayBuffer());

  const binDir = getBinDir();
  if (!existsSync(binDir)) {
    mkdirSync(binDir, { recursive: true });
  }

  const binaryPath = getBinaryPath();
  writeFileSync(binaryPath, binaryBuffer);

  // Make executable on Unix
  if (process.platform !== "win32") {
    chmodSync(binaryPath, 0o755);
  }

  console.log(`fff: Binary downloaded to ${binaryPath}`);
  return releaseTag;
}

/**
 * Fetch the latest release tag from GitHub
 */
async function fetchLatestReleaseTag(): Promise<string> {
  const url = `https://api.github.com/repos/${GITHUB_REPO}/releases/latest`;

  const response = await fetch(url, {
    headers: {
      Accept: "application/vnd.github.v3+json",
      "User-Agent": "fff-bun-client",
    },
  });

  if (!response.ok) {
    const allReleasesUrl = `https://api.github.com/repos/${GITHUB_REPO}/releases`;
    const allResponse = await fetch(allReleasesUrl, {
      headers: {
        Accept: "application/vnd.github.v3+json",
        "User-Agent": "fff-bun-client",
      },
    });

    if (!allResponse.ok) {
      throw new Error(`Failed to fetch releases: ${allResponse.status}`);
    }

    const releases = (await allResponse.json()) as Array<{ tag_name: string }>;
    if (releases.length === 0) {
      throw new Error("No releases found");
    }

    return releases[0].tag_name;
  }

  const release = (await response.json()) as { tag_name: string };
  return release.tag_name;
}

/**
 * Ensure the binary exists, downloading from GitHub if necessary.
 */
export async function ensureBinary(): Promise<string> {
  const existingPath = findBinary();
  if (existingPath) {
    return existingPath;
  }

  // Fallback: download from GitHub
  await downloadBinary();
  return getBinaryPath();
}
