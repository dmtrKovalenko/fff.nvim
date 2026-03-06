#!/usr/bin/env bun
import { existsSync, mkdirSync } from "node:fs";
import { homedir, platform } from "node:os";
import { join } from "node:path";
import { FileFinder } from "@ff-labs/fff-bun";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { getCursor } from "./cursor";
import { fileSuffix, formatGrepResults, type OutputMode } from "./output";

function parseArgs(argv: string[]) {
  const args = argv.slice(2);
  let basePath = process.cwd();
  let frecencyDbPath = "";
  let historyDbPath = "";

  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--frecency-db" && args[i + 1]) {
      frecencyDbPath = args[++i];
    } else if (args[i] === "--history-db" && args[i + 1]) {
      historyDbPath = args[++i];
    } else if (!args[i].startsWith("--")) {
      basePath = args[i];
    }
  }

  // Default to Neovim's standard data locations so the MCP server shares
  // frecency/history databases with the fff.nvim plugin. Falls back to
  // ~/.fff/ if the Neovim directories don't exist.
  if (!frecencyDbPath || !historyDbPath) {
    const home = homedir();
    const nvimCacheDir =
      platform() === "win32"
        ? join(home, "AppData", "Local", "nvim-data")
        : join(home, ".cache", "nvim");
    const nvimDataDir =
      platform() === "win32"
        ? join(home, "AppData", "Local", "nvim-data")
        : join(home, ".local", "share", "nvim");

    // Use Neovim paths if the parent dirs exist (plugin has been used)
    const useNvimPaths = existsSync(nvimCacheDir) || existsSync(nvimDataDir);

    if (!frecencyDbPath) {
      frecencyDbPath = useNvimPaths
        ? join(nvimCacheDir, "fff_nvim")
        : join(home, ".fff", "frecency.mdb");
    }
    if (!historyDbPath) {
      historyDbPath = useNvimPaths
        ? join(nvimDataDir, "fff_queries")
        : join(home, ".fff", "history.mdb");
    }

    // Ensure parent directories exist
    const frecencyParent = join(frecencyDbPath, "..");
    const historyParent = join(historyDbPath, "..");
    mkdirSync(frecencyParent, { recursive: true });
    mkdirSync(historyParent, { recursive: true });
  }

  return { basePath, frecencyDbPath, historyDbPath };
}

async function main() {
  const { basePath, frecencyDbPath, historyDbPath } = parseArgs(process.argv);
  await FileFinder.ensureLoaded();

  const createResult = FileFinder.create({
    basePath,
    frecencyDbPath,
    historyDbPath,
    warmupMmapCache: true,
    aiMode: true,
  });

  if (!createResult.ok) {
    process.stderr.write(`Failed to create FileFinder: ${createResult.error}\n`);
    process.exit(1);
  }

  const finder = createResult.value;
  const server = new McpServer(
    { name: "fff", version: "0.1.0" },
    {
      instructions: [
        "FFF is a fast file finder with frecency-ranked results (frequent/recent files first, git-dirty files boosted).",
        "",
        "## Which Tool Should I Use?",
        "",
        "- **grep**: DEFAULT tool. Searches file CONTENTS — definitions, usage, patterns. Use when you have a specific name or pattern.",
        "- **find_files**: Explores which files/modules exist for a topic. Use when you DON'T have a specific identifier or LOOKING FOR A FILE.",
        "- multi_grep: OR logic across multiple patterns. Use for case variants (e.g. ['PrepareUpload', 'prepare_upload']).",
        "",
        "## Workflow",
        "",
        "**Have a specific name?** (function, struct, type, variable) → grep it directly.",
        "**Exploring a topic?** (auth, uploads, quote service) → find_files first to discover relevant files/dirs, then grep.",
        "**Listing files?** (what files relate to X?) → find_files.",
        "",
        "Search for simple names, not code patterns:",
        "  ✅ 'ActorAuth' → finds definition + all usages",
        "  ❌ 'ctx.data::<ActorAuth>' → too specific, 0 results",
        "",
        "**0 results? SIMPLIFY immediately.** Drop qualifiers, path syntax, struct/fn prefixes. Just search the bare name.",
        "**After 2 greps, Read the top result.** Do not keep searching — read and understand the code.",
        "",
        "## Constraint Syntax",
        "",
        "For grep: constraints go INLINE in query string, prepended before search text.",
        "For multi_grep: constraints go in separate 'constraints' parameter.",
        "",
        "Prefer generic constraints:",
        "  ✅ '*.rs query'    → file type (broad)",
        "  ✅ 'src/ query'     → top-level dir (broad)",
        "  ❌ 'src/utils/helpers/ query' → too specific",
        "",
        "Available Constraints:",
        "  Extension: '*.ts', '*.{ts,tsx}'",
        "  Any child of directory use when know the exact dir name: 'src/' (trailing slash required)",
        "  Exclude: '!test/', '!*.test.ts', '!node_modules/'",
        "  Single file: src/utils/helpers.ts for grep to find occurances in a single file (use only when sure)",
        "",
        "Important:",
        "  - NEVER put filenames in query — only dir/extension constraints work",
        "  - To search a specific file, use Read tool instead",
        "",
        "## Default Exclusions",
        "",
        "If you see any massively populated irrelevant files for your next search exlude them like",
        "!tests/ - exlude all the children of tests folder",
        "!*.spec.ts - exclude all test files with .spec.ts extension",
        "",
        "## Output Format",
        "",
        "grep results auto-expand definitions with body context (struct fields, function signatures).",
        "This often provides enough information WITHOUT a follow-up Read call.",
        "Lines marked with | are definition body context. [def] marks definition files.",
        "→ Read suggestions point to the most relevant file — follow them when you need more context.",
      ].join("\n"),
    },
  );

  server.registerTool(
    "find_files",
    {
      title: "FFF Find Files",
      description:
        "Fuzzy file search by name. Searches FILE NAMES, not file contents. Use it when you need to find a file, not a definition." +
        "Use grep instead for searching code content (definitions, usage patterns). " +
        "Supports fuzzy matching, path prefixes ('src/'), and glob constraints ('name **/src/*.{ts,tsx} !test/'). " +
        "IMPORTANT: Keep queries SHORT — prefer 1-2 terms max. Multiple words are a waterfall (each narrows results), NOT OR. " +
        "If unsure, start broad with 1 term and refine.",
      inputSchema: {
        query: z
          .string()
          .describe("Fuzzy search query. Supports path prefixes and glob constraints."),
        maxResults: z.number().optional().describe("Max results (default 20)."),
      },
    },
    async ({ query, maxResults = 20 }) => {
      // Auto-retry with fewer terms if 3+ words return 0 results
      let result = finder.search(query, { pageSize: maxResults });

      if (!result.ok) {
        return {
          content: [{ type: "text" as const, text: `Error: ${result.error}` }],
          isError: true,
        };
      }

      const words = query.trim().split(/\s+/);
      if (result.value.items.length === 0 && words.length >= 3) {
        // Over-filtered — retry with first 2 terms
        const shorter = words.slice(0, 2).join(" ");
        const retry = finder.search(shorter, { pageSize: maxResults });
        if (retry.ok && retry.value.items.length > 0) {
          result = retry;
        }
      }

      const { items, totalMatched, totalFiles } = result.value;

      if (items.length === 0) {
        return {
          content: [
            {
              type: "text" as const,
              text: `0 results (${totalFiles} indexed)`,
            },
          ],
        };
      }

      const lines: string[] = [];

      // Detect exact filename match — strongest confidence signal
      const queryLower = query.toLowerCase().replace(/[*!]/g, "").trim();
      const topItem = items[0];
      const isExactMatch =
        topItem.fileName.toLowerCase() === queryLower ||
        topItem.relativePath.toLowerCase() === queryLower ||
        topItem.relativePath.toLowerCase().endsWith("/" + queryLower);

      if (isExactMatch) {
        lines.push(`→ Read ${topItem.relativePath} (exact match — this is the file, no further search needed)`);
      } else if (items.length <= 3) {
        lines.push(`→ Read ${topItem.relativePath} (best match — Read this file directly)`);
      }

      if (totalMatched > items.length) {
        lines.push(`${items.length}/${totalMatched} matches`);
      }

      for (const item of items) {
        lines.push(
          `${item.relativePath}${fileSuffix(item.gitStatus, item.totalFrecencyScore)}`,
        );
      }

      return {
        content: [{ type: "text" as const, text: lines.join("\n") }],
      };
    },
  );

  // regex matchin any common delimiters used in snake_case, camelCase, PascalCase to split words for better fuzzy matching when regex fails
  const DELIMITERS_REGEX = /[:\-_]/g;

  // Detect if a query contains regex metacharacters (after stripping constraint prefixes).
  // Constraint prefixes like '*.rs ' or 'src/ ' use * and / but are NOT regex.
  const CONSTRAINT_PREFIX_RE = /^(?:(?:[!*][^\s]*|[^\s]*\/)\s+)+/;
  const REGEX_INDICATORS = /\.\*|\.\+|\.\?|\\[swbdSWBD().[\]]|\[.+\]|\(.+\)|[^\\]\||\^\w/;
  function looksLikeRegex(query: string): boolean {
    const body = query.replace(CONSTRAINT_PREFIX_RE, "");
    return REGEX_INDICATORS.test(body);
  }

  function performGrep(
    query: string,
    mode: "plain" | "regex",
    maxResults: number = 50,
    cursorId?: string,
    outputMode: OutputMode = "content",
    context?: number,
  ): { content: { type: "text"; text: string }[]; isError?: boolean } {
    const nativeCursor = cursorId ? (getCursor(cursorId) ?? null) : null;

    const isUsage = outputMode === "usage";
    const matchesPerFile = outputMode === "files_with_matches" ? 1 : isUsage ? 8 : 10;
    const ctxLines = isUsage ? (context ?? 1) : (context ?? 0);
    // Request extra afterContext for definition auto-expansion (struct fields, fn bodies)
    // This lets us show definition bodies inline, eliminating follow-up Read calls
    const autoExpand = !isUsage && ctxLines === 0;
    const afterCtx = autoExpand ? 8 : ctxLines;

    const result = finder.liveGrep(query, {
      mode,
      maxMatchesPerFile: matchesPerFile,
      timeBudgetMs: 0,
      cursor: nativeCursor,
      beforeContext: ctxLines,
      afterContext: afterCtx,
    });

    if (!result.ok) {
      return {
        content: [{ type: "text" as const, text: `Error: ${result.error}` }],
        isError: true,
      };
    }

    const { items: allItems, regexFallbackError } = result.value;

    if (allItems.length === 0 && !nativeCursor) {
      // Auto-retry: when a multi-word query fails and the first word isn't a valid
      // constraint, the first word may be over-constraining the search.
      // Only auto-broaden when the result set is focused (≤10 matches) to avoid
      // flooding the model with overly broad results.
      const parts = query.trim().split(/\s+/);
      if (parts.length >= 2) {
        const firstWord = parts[0];
        const isValidConstraint =
          firstWord.startsWith("!") ||
          firstWord.startsWith("*") ||
          firstWord.endsWith("/");
        if (!isValidConstraint) {
          const restQuery = parts.slice(1).join(" ");
          const retryMode = looksLikeRegex(restQuery) ? "regex" : mode;
          const retryResult = finder.liveGrep(restQuery, {
            mode: retryMode,
            maxMatchesPerFile: matchesPerFile,
            timeBudgetMs: 0,
            beforeContext: ctxLines,
            afterContext: afterCtx,
          });
          if (
            retryResult.ok &&
            retryResult.value.items.length > 0 &&
            retryResult.value.items.length <= 10
          ) {
            const text = formatGrepResults(
              retryResult.value,
              outputMode,
              maxResults,
              retryResult.value.regexFallbackError,
              ctxLines > 0,
              autoExpand,
            );
            return {
              content: [
                {
                  type: "text" as const,
                  text: `0 matches for '${query}'. Auto-broadened to '${restQuery}':\n${text}`,
                },
              ],
            };
          }
        }
      }

      // Fallback: fuzzy grep for typo tolerance
      // using lower case without delimiters is important to get the best fuzzy results
      const fuzzyResult = finder.liveGrep(
        query.toLowerCase().replace(DELIMITERS_REGEX, ""),
        {
          mode: "fuzzy",
          maxMatchesPerFile: matchesPerFile,
          timeBudgetMs: 0,
        },
      );

      if (fuzzyResult.ok && fuzzyResult.value.items.length > 0) {
        const fuzzyItems = fuzzyResult.value.items.slice(0, 3);
        const lines: string[] = [];
        lines.push(`0 exact matches. ${fuzzyResult.value.totalMatched} approximate:`);
        let currentFile = "";
        for (const match of fuzzyItems) {
          if (match.relativePath !== currentFile) {
            currentFile = match.relativePath;
            lines.push(currentFile);
          }
          lines.push(` ${match.lineNumber}: ${match.lineContent}`);
        }
        return {
          content: [{ type: "text" as const, text: lines.join("\n") }],
        };
      }

      // Detect if the query looks like a specific file path constraint that failed
      const constraintParts = query.match(CONSTRAINT_PREFIX_RE);
      const constraintStr = constraintParts ? constraintParts[0].trim() : "";
      const looksLikeFilePath = constraintStr && /\.\w+$/.test(constraintStr) && !constraintStr.startsWith("*");

      // Suggest the longest identifier from the query as a broader term
      const body = query.replace(CONSTRAINT_PREFIX_RE, "").trim();
      const tokens = body.split(/[.:;<>()[\]{}\s]+/).filter((t) => t.length >= 3);
      const longest = tokens.reduce((a, b) => (a.length >= b.length ? a : b), "");

      let hint: string;
      if (looksLikeFilePath) {
        hint = ` Constraint '${constraintStr}' looks like a file path — use Read to search in a specific file, or '*.${constraintStr.split(".").pop()}' for extension filter.`;
      } else if (longest && longest.length < body.length) {
        hint = ` Try '${longest}'.`;
      } else {
        hint = " Try a broader term.";
      }

      return {
        content: [
          {
            type: "text" as const,
            text: `0 matches.${hint}`,
          },
        ],
      };
    }

    if (allItems.length === 0) {
      return {
        content: [{ type: "text" as const, text: `0 matches.` }],
      };
    }

    const text = formatGrepResults(
      result.value,
      outputMode,
      maxResults,
      regexFallbackError,
      ctxLines > 0,
      autoExpand,
    );

    return {
      content: [{ type: "text" as const, text }],
    };
  }

  server.registerTool(
    "grep",
    {
      title: "FFF Grep",
      description:
        "Search file contents for text or regex patterns. This is the DEFAULT search tool — use it first for all code searches. " +
        "Auto-detects regex (e.g. 'fn\\s+\\w+', 'load.*metadata') vs literal text. " +
        "Filter files with constraints (e.g. '*.rs query', 'src/ query'). " +
        "NEVER put filenames in the query — only directory constraints (ending with /) and extension filters (*.) work. " +
        "To search in a specific file, use Read tool instead. See server instructions for constraint syntax.",
      inputSchema: {
        query: z
          .string()
          .describe(
            "Search text or regex query with optional constraint prefixes (e.g. '*.ts MyFunction', '*.rs fn\\s+\\w+')." +
              " Matches within single lines only — use ONE specific term, not multiple words.",
          ),
        maxResults: z.number().optional().describe("Max matching lines (default 20)."),
        cursor: z
          .string()
          .optional()
          .describe(
            "Cursor from previous result. Only use if previous results weren't sufficient.",
          ),
        output_mode: z
          .enum(["content", "files_with_matches", "count", "usage"])
          .optional()
          .describe(
            "Output format (default 'content'). 'content' shows matching lines, 'files_with_matches' shows only file paths, 'usage' shows more matches per file with context lines — use when you need to understand HOW something is called/used.",
          ),
      },
    },
    async ({
      query,
      maxResults = 20,
      cursor: cursorId,
      output_mode: outputMode = "content",
    }) => {
      const mode = looksLikeRegex(query) ? "regex" : "plain";
      return performGrep(query, mode, maxResults, cursorId, outputMode);
    },
  );

  server.registerTool(
    "multi_grep",
    {
      title: "FFF Multi Grep",
      description:
        "Search file contents for lines matching ANY of multiple patterns (OR logic). " +
        "IMPORTANT: This returns files where ANY query matches, NOT all patterns. " +
        "Patterns are literal text — NEVER escape special characters (no \\( \\) \\. etc). " +
        "Faster than regex alternation for literal text. See server instructions for constraint syntax.",
      inputSchema: {
        patterns: z
          .union([z.array(z.string()).min(1), z.string()])
          .transform((v) => {
            if (Array.isArray(v)) return v;
            // Handle stringified JSON arrays from MCP clients
            if (typeof v === "string" && v.startsWith("[")) {
              try {
                const parsed = JSON.parse(v);
                if (Array.isArray(parsed)) return parsed as string[];
              } catch {
                // not valid JSON — treat as a single query
              }
            }
            return [v as string];
          })
          .describe(
            "Patterns to match (OR logic). Include all naming conventions: snake_case, PascalCase, camelCase.",
          ),
        constraints: z
          .string()
          .optional()
          .describe(
            "File constraints (e.g. '*.{ts,tsx} !test/'). ALWAYS provide when possible.",
          ),
        maxResults: z.number().optional().describe("Max matching lines (default 20)."),
        cursor: z
          .string()
          .optional()
          .describe(
            "Cursor from previous result. Only use if previous results weren't sufficient.",
          ),
        output_mode: z
          .enum(["content", "files_with_matches", "count", "usage"])
          .optional()
          .describe("Output format (default 'content')."),
        context: z.number().optional().describe("Context lines before/after each match."),
      },
    },
    async ({
      patterns,
      constraints,
      maxResults = 20,
      cursor: cursorId,
      output_mode: outputMode = "content",
      context,
    }) => {
      const nativeCursor = cursorId ? (getCursor(cursorId) ?? null) : null;

      const isUsageMulti = outputMode === "usage";
      const multiMatchesPerFile =
        outputMode === "files_with_matches" ? 1 : isUsageMulti ? 8 : 10;
      const multiCtx = isUsageMulti ? (context ?? 1) : (context ?? 0);
      const autoExpandMulti = !isUsageMulti && multiCtx === 0;
      const afterCtxMulti = autoExpandMulti ? 8 : multiCtx;

      const result = finder.multiGrep({
        patterns,
        constraints,
        maxMatchesPerFile: multiMatchesPerFile,
        timeBudgetMs: 0,
        cursor: nativeCursor,
        beforeContext: multiCtx,
        afterContext: afterCtxMulti,
      });

      if (!result.ok) {
        return {
          content: [{ type: "text" as const, text: `Error: ${result.error}` }],
          isError: true,
        };
      }

      const { items: allItems } = result.value;

      if (allItems.length === 0 && !nativeCursor) {
        for (const pat of patterns) {
          const plainResult = finder.liveGrep(
            constraints ? `${constraints} ${pat}` : pat,
            { mode: "plain", timeBudgetMs: 3000 },
          );
          if (plainResult.ok && plainResult.value.items.length > 0) {
            const text = formatGrepResults(
              plainResult.value,
              outputMode,
              maxResults,
              undefined,
              false,
              autoExpandMulti,
            );
            return {
              content: [
                {
                  type: "text" as const,
                  text: `0 multi-pattern matches. Plain grep fallback for "${pat}":\n${text}`,
                },
              ],
            };
          }
        }

        return {
          content: [
            {
              type: "text" as const,
              text: `0 matches.`,
            },
          ],
        };
      }

      if (allItems.length === 0) {
        return {
          content: [{ type: "text" as const, text: `0 matches.` }],
        };
      }

      const text = formatGrepResults(
        result.value,
        outputMode,
        maxResults,
        undefined,
        multiCtx > 0,
        autoExpandMulti,
      );

      return {
        content: [{ type: "text" as const, text }],
      };
    },
  );

  const transport = new StdioServerTransport();

  process.on("SIGINT", () => {
    finder.destroy();
    process.exit(0);
  });

  process.on("SIGTERM", () => {
    finder.destroy();
    process.exit(0);
  });

  await server.connect(transport);
}

main().catch((err) => {
  process.stderr.write(`Fatal error: ${err}\n`);
  process.exit(1);
});
