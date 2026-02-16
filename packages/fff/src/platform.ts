/**
 * Platform detection utilities for downloading the correct binary
 */

import { execSync } from "node:child_process";

/**
 * Get the platform triple (e.g., "x86_64-unknown-linux-gnu")
 */
export function getTriple(): string {
  const platform = process.platform;
  const arch = process.arch;

  let osName: string;
  if (platform === "darwin") {
    osName = "apple-darwin";
  } else if (platform === "linux") {
    osName = detectLinuxLibc();
  } else if (platform === "win32") {
    osName = "pc-windows-msvc";
  } else {
    throw new Error(`Unsupported platform: ${platform}`);
  }

  const archName = normalizeArch(arch);
  return `${archName}-${osName}`;
}

/**
 * Detect whether we're on musl or glibc Linux
 */
function detectLinuxLibc(): string {
  try {
    const lddOutput = execSync("ldd --version 2>&1", {
      encoding: "utf-8",
      timeout: 5000,
    });
    if (lddOutput.toLowerCase().includes("musl")) {
      return "unknown-linux-musl";
    }
  } catch {
    // ldd failed, assume glibc
  }
  return "unknown-linux-gnu";
}

/**
 * Normalize architecture name to Rust target format
 */
function normalizeArch(arch: string): string {
  switch (arch) {
    case "x64":
    case "amd64":
      return "x86_64";
    case "arm64":
      return "aarch64";
    case "arm":
      return "arm";
    default:
      throw new Error(`Unsupported architecture: ${arch}`);
  }
}

/**
 * Get the library file extension for the current platform
 */
export function getLibExtension(): "dylib" | "so" | "dll" {
  switch (process.platform) {
    case "darwin":
      return "dylib";
    case "win32":
      return "dll";
    default:
      return "so";
  }
}

/**
 * Get the library filename prefix (empty on Windows)
 */
export function getLibPrefix(): string {
  return process.platform === "win32" ? "" : "lib";
}

/**
 * Get the full library filename for the current platform
 */
export function getLibFilename(): string {
  const prefix = getLibPrefix();
  const ext = getLibExtension();
  return `${prefix}fff_c.${ext}`;
}
