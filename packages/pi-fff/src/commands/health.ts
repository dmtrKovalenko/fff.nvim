import type { FileFinder } from "@ff-labs/fff-node";

export function executeHealth(finder: FileFinder | null): string {
  if (!finder) {
    return "FFF: not initialized (will initialize on first search)";
  }

  const health = finder.healthCheck();
  if (!health.ok) {
    return `FFF health check failed: ${health.error}`;
  }

  const h = health.value;
  const lines = [
    `FFF v${h.version}`,
    "",
    `File Picker: ${h.filePicker.initialized ? "active" : "not initialized"}`,
  ];

  if (h.filePicker.initialized) {
    lines.push(`  Base path: ${h.filePicker.basePath ?? "unknown"}`);
    lines.push(`  Indexed files: ${h.filePicker.indexedFiles ?? 0}`);
    lines.push(`  Scanning: ${h.filePicker.isScanning ? "yes" : "no"}`);
  }

  lines.push("");
  lines.push(`Git: ${h.git.repositoryFound ? "detected" : "not found"}`);
  if (h.git.repositoryFound && h.git.workdir) {
    lines.push(`  Workdir: ${h.git.workdir}`);
  }

  lines.push("");
  lines.push(`Frecency DB: ${h.frecency.initialized ? "active" : "not initialized"}`);
  if (h.frecency.dbHealthcheck) {
    const kb = Math.round(h.frecency.dbHealthcheck.diskSize / 1024);
    lines.push(`  Size: ${kb}KB`);
  }

  lines.push(
    `Query Tracker: ${h.queryTracker.initialized ? "active" : "not initialized"}`,
  );
  if (h.queryTracker.dbHealthcheck) {
    const kb = Math.round(h.queryTracker.dbHealthcheck.diskSize / 1024);
    lines.push(`  Size: ${kb}KB`);
  }

  return lines.join("\n");
}
