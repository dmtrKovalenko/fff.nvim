#!/usr/bin/env bun
/**
 * Postinstall script - automatically downloads the native binary
 */

import { downloadBinary, findBinary, getInstalledHash } from "../src/download";

async function main() {
  // Check if binary already exists (dev build or previous download)
  const existing = findBinary();
  if (existing) {
    const hash = getInstalledHash();
    console.log(`fff: Native library found at ${existing}`);
    if (hash) {
      console.log(`fff: Version: ${hash}`);
    }
    return;
  }

  console.log("fff: Native library not found, downloading...");

  try {
    const hash = await downloadBinary();
    console.log(`fff: Native library installed successfully! (${hash})`);
  } catch (error) {
    console.error("fff: Failed to download native library:", error);
    console.error("");
    console.error("fff: You can build from source instead:");
    console.error("  cd node_modules/fff && cargo build --release -p fff-c");
    console.error("");
    console.error("fff: Or run `bunx fff download` after fixing network issues.");
    // Don't exit with error - allow install to complete
    // The error will surface when the user tries to use the library
  }
}

main();
