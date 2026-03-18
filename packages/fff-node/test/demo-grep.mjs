import { FileFinder } from "@ff-labs/fff-node";

const finder = FileFinder.create({ basePath: "~/dev/linux-root" });
if (!finder.ok) {
  throw new Error(`Failed to create FileFinder: ${finder.error}`);
}

const scan = await finder.value.waitForScan(5_000);
if (!scan.ok) {
  throw new Error(`Failed to wait for scan: ${scan.error}`);
}

// 5ms on m1 mac at the linux kernel repo root
const result = finder.value.grep("hardwre", { mode: "plain" });
for (const match of result.value.items) {
  console.log(`${match.relativePath}:${match.lineNumber}: ${match.lineContent}`);
}

console.log(`\n${result.value.totalMatched} matches across ${result.value.totalFilesSearched} files`);

finder.value.destroy();
