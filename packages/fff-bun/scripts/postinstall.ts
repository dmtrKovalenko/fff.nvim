#!/usr/bin/env bun
/**
 * Postinstall script - ensures the native binary is available
 *
 * Resolution order:
 * 1. Platform-specific npm package (installed via optionalDependencies)
 * 2. Local dev build (target/release or target/debug)
 * 3. Fallback: download from GitHub releases
 */

import { findBinary, downloadBinary } from "../src/download";
import { getNpmPackageName } from "../src/platform";

async function main() {
  // Check if binary is already available (npm package or dev build)
  const existing = findBinary();
  if (existing) {
    console.log(`fff: Native library found at ${existing}`);
    return;
  }

  // Binary not found via npm package - try downloading from GitHub as fallback
  let packageName: string;
  try {
    packageName = getNpmPackageName();
  } catch {
    packageName = "unknown";
  }

  console.log(
    `fff: Platform package ${packageName} not found, falling back to GitHub download...`
  );

  try {
    const tag = await downloadBinary();
    console.log(`fff: Native library installed successfully! (${tag})`);
  } catch (error) {
    console.error("fff: Failed to download native library:", error);
    console.error("");
    console.error("fff: You can build from source instead:");
    console.error("  cargo build --release -p fff-c");
    console.error("");
    console.error(
      "fff: Or run `bunx fff download` after fixing network issues."
    );
    // Don't exit with error - allow install to complete
    // The error will surface when the user tries to use the library
  }
}

main();
