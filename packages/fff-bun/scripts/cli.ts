#!/usr/bin/env bun
/**
 * CLI tool for fff package management
 *
 * Usage:
 *   bunx fff download [tag]     - Download native binary from GitHub
 *   bunx fff info               - Show platform and binary info
 */

import { 
  downloadBinary, 
  getBinaryPath, 
  findBinary, 
} from "../src/download";
import { getTriple, getLibExtension, getLibFilename, getNpmPackageName } from "../src/platform";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const command = args[0];

interface PackageJson {
  version: string;
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
      const tag = args[1];
      console.log("fff: Downloading native library from GitHub...");
      try {
        const resolvedTag = await downloadBinary(tag);
        console.log(`fff: Download complete! (${resolvedTag})`);
      } catch (error) {
        console.error("fff: Download failed:", error);
        process.exit(1);
      }
      break;
    }

    case "info": {
      const pkg = await getPackageInfo();
      let npmPackage: string;
      try {
        npmPackage = getNpmPackageName();
      } catch {
        npmPackage = "unsupported";
      }
      
      console.log("fff - Fast File Finder");
      console.log(`Package version: ${pkg.version}`);
      console.log("");
      console.log("Platform Information:");
      console.log(`  Triple: ${getTriple()}`);
      console.log(`  Extension: ${getLibExtension()}`);
      console.log(`  Library name: ${getLibFilename()}`);
      console.log(`  npm package: ${npmPackage}`);
      console.log("");
      console.log("Binary Status:");
      const existing = findBinary();
      if (existing) {
        console.log(`  Found: ${existing}`);
      } else {
        console.log(`  Not found`);
        console.log(`  Expected path: ${getBinaryPath()}`);
        console.log(`  Try: bun add ${npmPackage}`);
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
      console.log("  bunx fff download [tag]    Download native binary from GitHub (fallback)");
      console.log("  bunx fff info              Show platform and binary info");
      console.log("  bunx fff version           Show version");
      console.log("  bunx fff help              Show this help message");
      console.log("");
      console.log("Examples:");
      console.log("  bunx fff download          Download latest binary from GitHub");
      console.log("  bunx fff download abc1234  Download specific release tag");
      console.log("");
      console.log("Note: Binaries are normally provided via platform-specific npm packages.");
      console.log("The download command is a fallback for when those aren't available.");
      break;
    }
  }
}

main();
