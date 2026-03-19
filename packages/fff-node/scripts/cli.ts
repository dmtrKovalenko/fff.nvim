/**
 * CLI tool for fff-node package management
 *
 * Usage:
 *   npx @ff-labs/fff-node download [tag]  - Download native binary from GitHub
 *   npx @ff-labs/fff-node info            - Show platform and binary info
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { downloadBinary, findBinary, getBinaryPath } from "../src/binary.js";
import {
  getLibExtension,
  getLibFilename,
  getNpmPackageName,
  getTriple,
} from "../src/platform.js";

const args = process.argv.slice(2);
const command = args[0];

interface PackageJson {
  version: string;
}

function getPackageInfo(): PackageJson {
  const currentDir = dirname(fileURLToPath(import.meta.url));
  const packageJsonPath = join(currentDir, "..", "package.json");

  try {
    return JSON.parse(readFileSync(packageJsonPath, "utf-8"));
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
      const pkg = getPackageInfo();
      let npmPackage: string;
      try {
        npmPackage = getNpmPackageName();
      } catch {
        npmPackage = "unsupported";
      }

      console.log("fff - Fast File Finder (Node.js)");
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
        console.log("  Not found");
        console.log(`  Expected path: ${getBinaryPath()}`);
        console.log(`  Try: npm add ${npmPackage}`);
      }
      break;
    }

    case "version":
    case "--version":
    case "-v": {
      const pkg = getPackageInfo();
      console.log(pkg.version);
      break;
    }

    default: {
      const pkg = getPackageInfo();
      console.log(`fff - Fast File Finder CLI (Node.js) v${pkg.version}`);
      console.log("");
      console.log("Usage:");
      console.log(
        "  npx @ff-labs/fff-node download [tag]  Download native binary from GitHub (fallback)",
      );
      console.log(
        "  npx @ff-labs/fff-node info             Show platform and binary info",
      );
      console.log("  npx @ff-labs/fff-node version          Show version");
      console.log("  npx @ff-labs/fff-node help             Show this help message");
      console.log("");
      console.log("Examples:");
      console.log(
        "  npx @ff-labs/fff-node download          Download latest binary from GitHub",
      );
      console.log(
        "  npx @ff-labs/fff-node download abc1234  Download specific release tag",
      );
      console.log("");
      console.log(
        "Note: Binaries are normally provided via platform-specific npm packages.",
      );
      console.log("The download command is a fallback for when those aren't available.");
      break;
    }
  }
}

main();
