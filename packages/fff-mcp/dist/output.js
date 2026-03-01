import { storeCursor } from "./cursor";
/** Frecency score → single-token word. Empty for low-scoring files. */
export function frecencyWord(score) {
    if (score >= 100)
        return "hot";
    if (score >= 50)
        return "warm";
    if (score >= 10)
        return "frequent";
    return "";
}
/** Git status → single-token word. Empty for clean files. */
export function gitWord(status) {
    switch (status) {
        case "modified":
            return "modified";
        case "untracked":
            return "untracked";
        case "added":
        case "staged_new":
            return "staged";
        case "deleted":
            return "deleted";
        case "renamed":
            return "renamed";
        case "conflicted":
            return "conflicted";
        default:
            return "";
    }
}
/** Build " - hot git:modified" style suffix. Empty when nothing to report. */
export function fileSuffix(gitStatus, frecencyScore) {
    const f = frecencyWord(frecencyScore);
    const g = gitWord(gitStatus);
    if (!f && !g)
        return "";
    const parts = [];
    if (f)
        parts.push(f);
    if (g)
        parts.push(`git:${g}`);
    return ` - ${parts.join(" ")}`;
}
export function formatGrepResults(result, outputMode, maxResults, regexFallbackError) {
    const { items: allItems, totalMatched, nextCursor } = result;
    const items = allItems.slice(0, maxResults);
    if (outputMode === "files_with_matches") {
        // Group by file, show unique file paths
        const fileMap = new Map();
        for (const match of items) {
            if (!fileMap.has(match.relativePath)) {
                fileMap.set(match.relativePath, {
                    gitStatus: match.gitStatus,
                    frecencyScore: match.totalFrecencyScore,
                });
            }
        }
        const lines = [];
        lines.push(`${fileMap.size} files matched`);
        for (const [path, meta] of fileMap) {
            lines.push(`${path}${fileSuffix(meta.gitStatus, meta.frecencyScore)}`);
        }
        if (nextCursor) {
            lines.push(`\nMore results exist. Evaluate these results first before paginating. cursor: ${storeCursor(nextCursor)}`);
        }
        return lines.join("\n");
    }
    if (outputMode === "count") {
        // Group by file, count matches per file
        const countMap = new Map();
        for (const match of items) {
            countMap.set(match.relativePath, (countMap.get(match.relativePath) ?? 0) + 1);
        }
        const totalCount = items.length;
        const lines = [];
        lines.push(`${totalCount} matches in ${countMap.size} files`);
        for (const [path, count] of countMap) {
            lines.push(`${path}: ${count}`);
        }
        if (nextCursor) {
            lines.push(`\nMore results exist. Evaluate these results first before paginating. cursor: ${storeCursor(nextCursor)}`);
        }
        return lines.join("\n");
    }
    // "content" mode (default)
    const lines = [];
    if (regexFallbackError) {
        lines.push(`! regex failed: ${regexFallbackError}, using literal match`);
    }
    if (totalMatched > items.length) {
        lines.push(`${items.length}/${totalMatched} matches shown`);
    }
    let currentFile = "";
    for (const match of items) {
        if (match.relativePath !== currentFile) {
            currentFile = match.relativePath;
            lines.push(`${currentFile}${fileSuffix(match.gitStatus, match.totalFrecencyScore)}`);
        }
        // Context before
        if (match.contextBefore && match.contextBefore.length > 0) {
            const startLine = match.lineNumber - match.contextBefore.length;
            for (let i = 0; i < match.contextBefore.length; i++) {
                lines.push(` ${startLine + i}-${match.contextBefore[i]}`);
            }
        }
        // Match line (use : separator to distinguish from context)
        if (match.contextBefore?.length || match.contextAfter?.length) {
            lines.push(` ${match.lineNumber}:${match.lineContent}`);
        }
        else {
            lines.push(` ${match.lineNumber}: ${match.lineContent}`);
        }
        // Context after
        if (match.contextAfter && match.contextAfter.length > 0) {
            const startLine = match.lineNumber + 1;
            for (let i = 0; i < match.contextAfter.length; i++) {
                lines.push(` ${startLine + i}-${match.contextAfter[i]}`);
            }
            lines.push("--");
        }
    }
    if (nextCursor) {
        lines.push(`\nMore results exist. Evaluate these results first before paginating. cursor: ${storeCursor(nextCursor)}`);
    }
    return lines.join("\n");
}
