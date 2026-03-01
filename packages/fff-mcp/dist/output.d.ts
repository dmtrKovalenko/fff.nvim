import type { GrepResult } from "@ff-labs/fff-bun";
/** Frecency score → single-token word. Empty for low-scoring files. */
export declare function frecencyWord(score: number): string;
/** Git status → single-token word. Empty for clean files. */
export declare function gitWord(status: string): string;
/** Build " - hot git:modified" style suffix. Empty when nothing to report. */
export declare function fileSuffix(gitStatus: string, frecencyScore: number): string;
export type OutputMode = "content" | "files_with_matches" | "count";
export declare function formatGrepResults(result: GrepResult, outputMode: OutputMode, maxResults: number, regexFallbackError?: string): string;
//# sourceMappingURL=output.d.ts.map