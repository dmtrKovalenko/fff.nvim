#!/usr/bin/env bun
/**
 * CLI tool for fff package management
 *
 * Usage:
 *   bunx fff download [hash]    - Download native binary
 *   bunx fff info               - Show platform and binary info
 *   bunx fff check              - Check for updates
 */

import { 
  downloadBinary, 
  getBinaryPath, 
  findBinary, 
  getInstalledHash,
  checkForUpdate 
} from "../src/download";
import { getTriple, getLibExtension, getLibFilename } from "../src/platform";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const command = args[0];

interface PackageJson {
  version: string;
  nativeBinaryHash?: string;
}

async function getPackageInfo(): Promise<PackageJson> {
  const currentDir = dirname(fileURLToPath(import.meta.url));
  const packageJsonPath = join(currentDir, "..", "package.json");
  
  try {
    return await Bun.file(packageJsonPath).json();
  } catch {
    return { version: "unknown" };
  }
}

async function main() {
  switch (command) {
    case "download": {
      const hash = args[1];
      console.log("fff: Downloading native library...");
      try {
        const resolvedHash = await downloadBinary(hash);
        console.log(`fff: Download complete! (${resolvedHash})`);
      } catch (error) {
        console.error("fff: Download failed:", error);
        process.exit(1);
      }
      break;
    }

    case "check": {
      console.log("fff: Checking for updates...");
      try {
        const { currentHash, latestHash, updateAvailable } = await checkForUpdate();
        console.log(`  Installed: ${currentHash || "not installed"}`);
        console.log(`  Latest:    ${latestHash}`);
        if (updateAvailable) {
          console.log("");
          console.log("  Update available! Run: bunx fff download");
        } else {
          console.log("");
          console.log("  You're up to date!");
        }
      } catch (error) {
        console.error("fff: Failed to check for updates:", error);
        process.exit(1);
      }
      break;
    }

    case "info": {
      const pkg = await getPackageInfo();
      const installedHash = await getInstalledHash();
      
      console.log("fff - Fast File Finder");
      console.log(`Package version: ${pkg.version}`);
      console.log(`Binary hash: ${installedHash || "not installed"}`);
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
      const pkg = await getPackageInfo();
      console.log(pkg.version);
      break;
    }

    case "help":
    case "--help":
    case "-h":
    default: {
      const pkg = await getPackageInfo();
      console.log(`fff - Fast File Finder CLI v${pkg.version}`);
      console.log("");
      console.log("Usage:");
      console.log("  bunx fff download [hash]   Download native binary");
      console.log("  bunx fff check             Check for updates");
      console.log("  bunx fff info              Show platform and binary info");
      console.log("  bunx fff version           Show version");
      console.log("  bunx fff help              Show this help message");
      console.log("");
      console.log("Examples:");
      console.log("  bunx fff download          Download binary for configured hash");
      console.log("  bunx fff download latest   Download latest release");
      console.log("  bunx fff download abc1234  Download specific commit hash");
      break;
    }
  }
}

main();
