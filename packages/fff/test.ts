/**
 * Test script for fff package
 *
 * Run with: bun packages/fff/test.ts
 */

import { FileFinder } from "./src/index";
import { resolve, dirname } from "path";

async function main() {
  console.log("=== fff Test Script ===\n");

  // Check if library is available
  console.log("Checking library availability...");
  const available = FileFinder.isAvailable();
  console.log(`Library available: ${available}\n`);

  if (!available) {
    console.error("Native library not found!");
    console.error("Build it with: cargo build --release -p fff-c");
    process.exit(1);
  }

  // Health check (before init)
  console.log("Health check (before init):");
  const healthBefore = FileFinder.healthCheck();
  if (healthBefore.ok) {
    console.log(`  Version: ${healthBefore.value.version}`);
    console.log(`  Git available: ${healthBefore.value.git.available}`);
    console.log(`  File picker initialized: ${healthBefore.value.filePicker.initialized}`);
  } else {
    console.error(`  Error: ${healthBefore.error}`);
  }
  console.log();

  // Initialize with the root project directory to test on more files
  const testDir = resolve(dirname(import.meta.path), "../..");
  console.log(`Initializing with base path: ${testDir}`);

  const initResult = FileFinder.init({
    basePath: testDir,
  });

  if (!initResult.ok) {
    console.error(`Init failed: ${initResult.error}`);
    process.exit(1);
  }
  console.log("Initialization successful!\n");

  // Wait for scan with polling to show progress
  console.log("Waiting for initial scan...");
  const startTime = Date.now();
  let lastCount = 0;
  
  while (FileFinder.isScanning()) {
    const progress = FileFinder.getScanProgress();
    if (progress.ok && progress.value.scannedFilesCount !== lastCount) {
      lastCount = progress.value.scannedFilesCount;
      console.log(`  Scanning: ${lastCount} files...`);
    }
    await new Promise((r) => setTimeout(r, 100));
    
    if (Date.now() - startTime > 30000) {
      console.error("  Scan timeout after 30s");
      break;
    }
  }

  // Get final scan progress
  const progress = FileFinder.getScanProgress();
  if (progress.ok) {
    console.log(`Scan complete: ${progress.value.scannedFilesCount} files indexed`);
    console.log(`Scan time: ${Date.now() - startTime}ms`);
  }
  console.log();

  // Search test
  console.log("Searching for 'lib.rs'...");
  const searchResult = FileFinder.search("lib.rs", { pageSize: 5 });

  if (searchResult.ok) {
    console.log(`Found ${searchResult.value.totalMatched} matches (showing first 5):\n`);
    for (let i = 0; i < searchResult.value.items.length; i++) {
      const item = searchResult.value.items[i];
      const score = searchResult.value.scores[i];
      console.log(`  ${item.relativePath}`);
      console.log(`    Score: ${score.total} (base: ${score.baseScore}, filename: ${score.filenameBonus})`);
      console.log(`    Git: ${item.gitStatus}`);
    }
  } else {
    console.error(`Search failed: ${searchResult.error}`);
  }
  console.log();

  // Search with different query
  console.log("Searching for 'package.json'...");
  const searchResult2 = FileFinder.search("package.json", { pageSize: 3 });

  if (searchResult2.ok) {
    console.log(`Found ${searchResult2.value.totalMatched} matches:\n`);
    for (const item of searchResult2.value.items) {
      console.log(`  ${item.relativePath}`);
    }
  } else {
    console.error(`Search failed: ${searchResult2.error}`);
  }
  console.log();

  // Health check (after init)
  console.log("Health check (after init):");
  const healthAfter = FileFinder.healthCheck();
  if (healthAfter.ok) {
    console.log(`  File picker initialized: ${healthAfter.value.filePicker.initialized}`);
    console.log(`  Base path: ${healthAfter.value.filePicker.basePath}`);
    console.log(`  Indexed files: ${healthAfter.value.filePicker.indexedFiles}`);
    if (healthAfter.value.git.repositoryFound) {
      console.log(`  Git workdir: ${healthAfter.value.git.workdir}`);
    }
  }
  console.log();

  // Cleanup
  console.log("Cleaning up...");
  const destroyResult = FileFinder.destroy();
  if (destroyResult.ok) {
    console.log("Cleanup successful!");
  } else {
    console.error(`Cleanup failed: ${destroyResult.error}`);
  }

  console.log("\n=== Test Complete ===");
}

main().catch(console.error);
