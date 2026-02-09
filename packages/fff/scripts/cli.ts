#!/usr/bin/env bun
/**
 * CLI tool for fff package management
 *
 * Usage:
 *   bunx fff download [version]  - Download native binary
 *   bunx fff info                - Show platform and binary info
 */

import { downloadBinary, binaryExists, getBinaryPath, findBinary } from "../src/download";
import { getTriple, getLibExtension, getLibFilename } from "../src/platform";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const command = args[0];

async function getPackageVersion(): Promise<string> {
  const currentDir = dirname(fileURLToPath(import.meta.url));
  const packageJsonPath = join(currentDir, "..", "package.json");
  
  try {
    const pkg = await Bun.file(packageJsonPath).json();
    return pkg.version;
  } catch {
    return "unknown";
  }
}

async function main() {
  switch (command) {
    case "download": {
      const version = args[1];
      console.log("fff: Downloading native library...");
      try {
        await downloadBinary(version);
        console.log("fff: Download complete!");
      } catch (error) {
        console.error("fff: Download failed:", error);
        process.exit(1);
      }
      break;
    }

    case "info": {
      const version = await getPackageVersion();
      console.log("fff - Fast File Finder");
      console.log(`Version: ${version}`);
      console.log("");
      console.log("Platform Information:");
      console.log(`  Triple: ${getTriple()}`);
      console.log(`  Extension: ${getLibExtension()}`);
      console.log(`  Library name: ${getLibFilename()}`);
      console.log("");
      console.log("Binary Status:");
      const existing = findBinary();
      if (existing) {
        console.log(`  Found: ${existing}`);
      } else {
        console.log(`  Not found`);
        console.log(`  Expected path: ${getBinaryPath()}`);
      }
      break;
    }

    case "version":
    case "--version":
    case "-v": {
      const version = await getPackageVersion();
      console.log(version);
      break;
    }

    case "help":
    case "--help":
    case "-h":
    default: {
      const version = await getPackageVersion();
      console.log(`fff - Fast File Finder CLI v${version}`);
      console.log("");
      console.log("Usage:");
      console.log("  bunx fff download [version]  Download native binary");
      console.log("  bunx fff info                Show platform and binary info");
      console.log("  bunx fff version             Show version");
      console.log("  bunx fff help                Show this help message");
      console.log("");
      console.log("Examples:");
      console.log("  bunx fff download            Download latest binary");
      console.log("  bunx fff download abc1234    Download specific version");
      break;
    }
  }
}

main();
