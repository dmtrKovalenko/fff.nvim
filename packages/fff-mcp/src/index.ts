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
        "FFF is a fast file finder and content search engine. All results are ranked by frecency (recently/frequently accessed files first taking into account current git status/recency of edit in git/fs status/previous queries match and many more).",
        "",
        "Workflow: use find_files to discover file names and directory structure, then grep/regex_grep/multi_grep to search content.",
        "",
        "All results will be marked if the file is frequently modified, tracked by git, etc.",
        "",
        "## Which Tool Should I Use?",
        "",
        "┌─ Do you know exact file/class/function names?",
        "│  └─ YES → find_files (fastest)",
        "│",
        "├─ Are you looking for exact literal text?",
        "│  ├─ Single term → grep",
        "│  └─ Multiple alternatives (OR) → multi_grep",
        "│",
        "├─ Need pattern matching (wildcards, word boundaries)?",
        "│  └─ YES → regex_grep",
        "│",
        "├─ Exploring a codebase area?",
        "│  └─ Use find_files FIRST to discover scope",
        "│     Then grep within that scope",
        "│",
        "└─ Searching for architectural concepts (e.g., 'X optimized for Y')?",
        "   └─ Step 1: find_files with arch/module keywords",
        "      Step 2: grep with constraints targeting those files",
        "",
        "## Search Philosophy",
        "",
        "Results are ranked by frecency — the most relevant files appear first without constraints.",
        "**Prefer simple, broad queries.** Only add constraints when you have too many results, not preemptively.",
        "",
        "GOOD: Start broad, narrow only if needed:",
        "  1. grep(pattern: 'swscale_internal')              → search everything, frecency sorts it",
        "  2. grep(pattern: '*.c swscale_internal')           → too many results? add extension filter",
        "",
        "BAD: Over-constrained from the start:",
        "  ❌ grep(pattern: 'libswscale/aarch64/ *.{S,c} !test/ swscale_internal') → too specific, fragile",
        "",
        "## Constraint Syntax",
        "",
        "For grep/regex_grep: constraints go INLINE in the pattern string, prepended before the search text.",
        "For multi_grep: constraints go in the separate 'constraints' parameter (NOT in patterns array).",
        "",
        "Prefer generic constraints over specific ones:",
        "  ✅ '*.rs pattern'           → filter by file type (broad, reliable)",
        "  ✅ 'src/ pattern'            → filter by top-level directory (broad)",
        "  ❌ 'src/utils/helpers/ pattern' → too specific, breaks if structure changes",
        "",
        "### Available Constraints:",
        "  Extension: '*.ts', '*.{ts,tsx}'",
        "  Directory: 'src/' (trailing slash required — top-level or single-segment preferred)",
        "  Exclude:   '!test/', '!*.test.ts', '!node_modules/'",
        "",
        "### Important:",
        "  - grep is LITERAL text search. Use regex_grep for patterns like .* or \\b",
        "  - To search in a specific file, use the Read tool instead of constraining grep",
        "",
        "Shared parameters across grep tools:",
        "  maxResults: max matching lines to return (default 50). Increase if you need more.",
        "  output_mode: 'content' (default) shows matching lines with file:line, 'files_with_matches' shows file paths only, 'count' shows match counts per file.",
        "  context: number of lines before/after each match (only with output_mode 'content').",
        "  cursor: only use if previous results weren't sufficient.",
      ].join("\n"),
    },
  );

  server.registerTool(
    "find_files",
    {
      title: "FFF Find Files",
      description:
        "Fuzzy file search by name. Use this first to discover file names and directory structure before grepping content. " +
        "Supports fuzzy matching, path prefixes ('src/'), file:line syntax, and glob constraints ('name **/src/*.{ts,tsx} !test/'). " +
        "IMPORTANT: Keep queries SHORT — prefer 1-2 terms max. Multiple words are a waterfall (each narrows results), NOT OR. " +
        "❌ 'aarch64 neon rgba yuv420' → over-filtered, 0 results. " +
        "✅ 'aarch64 yuv' → focused, finds relevant files. " +
        "If unsure, start broad with 1 term and refine.",
      inputSchema: {
        query: z
          .string()
          .describe("Fuzzy search query. Supports path prefixes and glob constraints."),
        maxResults: z.number().optional().describe("Max results (default 20)."),
      },
    },
    async ({ query, maxResults = 20 }) => {
      const result = finder.search(query, { pageSize: maxResults });

      if (!result.ok) {
        return {
          content: [{ type: "text", text: `Error: ${result.error}` }],
          isError: true,
        };
      }

      const { items, totalMatched, totalFiles } = result.value;

      if (items.length === 0) {
        return {
          content: [
            {
              type: "text",
              text: `0 results (${totalFiles} indexed)`,
            },
          ],
        };
      }

      const lines: string[] = [];
      const more = totalMatched > items.length ? `, ${totalMatched} total` : "";
      lines.push(`${items.length}/${totalFiles} files${more}`);

      for (const item of items) {
        lines.push(
          `${item.relativePath}${fileSuffix(item.gitStatus, item.totalFrecencyScore)}`,
        );
      }

      return {
        content: [{ type: "text", text: lines.join("\n") }],
      };
    },
  );

  // regex matchin any common delimiters used in snake_case, camelCase, PascalCase to split words for better fuzzy matching when regex fails
  const DELIMITERS_REGEX = /[:\-_]/g;

  function performGrep(
    pattern: string,
    mode: "plain" | "regex",
    maxResults: number = 50,
    cursorId?: string,
    outputMode: OutputMode = "content",
    context?: number,
  ) {
    const nativeCursor = cursorId ? (getCursor(cursorId) ?? null) : null;

    const result = finder.liveGrep(pattern, {
      mode,
      maxMatchesPerFile: 30,
      timeBudgetMs: 0,
      cursor: nativeCursor,
      beforeContext: context,
      afterContext: context,
    });

    if (!result.ok) {
      return {
        content: [{ type: "text", text: `Error: ${result.error}` }],
        isError: true,
      };
    }

    const { items: allItems, regexFallbackError } = result.value;

    if (allItems.length === 0 && !nativeCursor) {
      // Fallback: fuzzy grep for typo tolerance
      // using lower case without delimiters is important to get the best fuzzy results
      const fuzzyResult = finder.liveGrep(
        pattern.toLowerCase().replace(DELIMITERS_REGEX, ""),
        {
          mode: "fuzzy",
          timeBudgetMs: 0,
        },
      );

      if (fuzzyResult.ok && fuzzyResult.value.items.length > 0) {
        const fuzzyLimit = Math.max(1, Math.floor(maxResults / 2));
        const fuzzyItems = fuzzyResult.value.items.slice(0, fuzzyLimit);
        const lines: string[] = [];
        lines.push(
          `0 exact matches. Fuzzy fallback found ${fuzzyResult.value.totalMatched} approximate matches (showing ${fuzzyItems.length}):`,
        );
        let currentFile = "";
        for (const match of fuzzyItems) {
          if (match.relativePath !== currentFile) {
            currentFile = match.relativePath;
            lines.push(
              `${currentFile}${fileSuffix(match.gitStatus, match.totalFrecencyScore)}`,
            );
          }
          lines.push(` ${match.lineNumber}: ${match.lineContent}`);
        }
        lines.push(
          `\nThese are approximate matches — refine your query or use find_files to discover exact names before grepping.`,
        );
        return {
          content: [{ type: "text", text: lines.join("\n") }],
        };
      }

      return {
        content: [
          {
            type: "text",
            text: `0 matches. Try find_files to discover file/symbol names first, then grep with exact terms.`,
          },
        ],
      };
    }

    if (allItems.length === 0) {
      return {
        content: [{ type: "text", text: `0 matches.` }],
      };
    }

    const text = formatGrepResults(
      result.value,
      outputMode,
      maxResults,
      regexFallbackError,
    );

    return {
      content: [{ type: "text", text }],
    };
  }

  server.registerTool(
    "grep",
    {
      title: "FFF Grep",
      description:
        "Search file contents for exact literal text. " +
        "Matches EXACT strings only — 'quote format date' won't match unless that exact phrase appears. " +
        "NEVER use regex syntax (.* \\( \\b etc) — this is LITERAL search. Use regex_grep for patterns. " +
        "NEVER put filenames in the pattern — only directory constraints (ending with /) and extension filters (*.) work. " +
        "To search in a specific file, use Read tool instead. " +
        "See server instructions for constraint syntax.",
      inputSchema: {
        pattern: z
          .string()
          .describe(
            "Exact search text with optional constraint prefixes (e.g. '*.ts MyFunction').",
          ),
        maxResults: z.number().optional().describe("Max matching lines (default 50)."),
        cursor: z
          .string()
          .optional()
          .describe(
            "Cursor from previous result. Only use if previous results weren't sufficient.",
          ),
        output_mode: z
          .enum(["content", "files_with_matches", "count"])
          .optional()
          .describe("Output format (default 'content')."),
        context: z.number().optional().describe("Context lines before/after each match."),
      },
    },
    async ({
      pattern,
      maxResults = 50,
      cursor: cursorId,
      output_mode: outputMode = "content",
      context,
    }) => {
      return performGrep(pattern, "plain", maxResults, cursorId, outputMode, context);
    },
  );

  // -------------------------------------------------------------------------
  // Tool: fff_regex_grep — regex pattern matching
  // -------------------------------------------------------------------------
  server.registerTool(
    "regex_grep",
    {
      title: "FFF Regex Grep",
      description:
        "Search file contents with regex patterns (e.g. 'fn\\s+\\w+', 'import.*from'). " +
        "Falls back to literal match if regex fails to compile. See server instructions for constraint syntax.",
      inputSchema: {
        pattern: z
          .string()
          .describe(
            "Regex pattern with optional constraint prefixes (e.g. '*.rs fn\\s+\\w+').",
          ),
        maxResults: z.number().optional().describe("Max matching lines (default 50)."),
        cursor: z
          .string()
          .optional()
          .describe(
            "Cursor from previous result. Only use if previous results weren't sufficient.",
          ),
        output_mode: z
          .enum(["content", "files_with_matches", "count"])
          .optional()
          .describe("Output format (default 'content')."),
        context: z.number().optional().describe("Context lines before/after each match."),
      },
    },
    async ({
      pattern,
      maxResults = 50,
      cursor: cursorId,
      output_mode: outputMode = "content",
      context,
    }) => {
      return performGrep(pattern, "regex", maxResults, cursorId, outputMode, context);
    },
  );

  server.registerTool(
    "multi_grep",
    {
      title: "FFF Multi Grep",
      description:
        "Search file contents for lines matching ANY of multiple patterns (OR logic). " +
        "IMPORTANT: This returns files where ANY pattern matches, NOT all patterns. " +
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
                // not valid JSON — treat as a single pattern
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
        maxResults: z.number().optional().describe("Max matching lines (default 50)."),
        cursor: z
          .string()
          .optional()
          .describe(
            "Cursor from previous result. Only use if previous results weren't sufficient.",
          ),
        output_mode: z
          .enum(["content", "files_with_matches", "count"])
          .optional()
          .describe("Output format (default 'content')."),
        context: z.number().optional().describe("Context lines before/after each match."),
      },
    },
    async ({
      patterns,
      constraints,
      maxResults = 50,
      cursor: cursorId,
      output_mode: outputMode = "content",
      context,
    }) => {
      const nativeCursor = cursorId ? (getCursor(cursorId) ?? null) : null;

      const result = finder.multiGrep({
        patterns,
        constraints,
        maxMatchesPerFile: maxResults / 2,
        timeBudgetMs: 0,
        cursor: nativeCursor,
        beforeContext: context,
        afterContext: context,
      });

      if (!result.ok) {
        return {
          content: [{ type: "text", text: `Error: ${result.error}` }],
          isError: true,
        };
      }

      const { items: allItems } = result.value;

      if (allItems.length === 0 && !nativeCursor) {
        // Fallback: try each pattern individually with plain grep
        for (const pat of patterns) {
          const plainResult = finder.liveGrep(
            constraints ? `${constraints} ${pat}` : pat,
            { mode: "plain", timeBudgetMs: 3000 },
          );
          if (plainResult.ok && plainResult.value.items.length > 0) {
            const text = formatGrepResults(plainResult.value, outputMode, maxResults);
            return {
              content: [
                {
                  type: "text",
                  text: `0 multi-pattern matches. Plain grep fallback for "${pat}":\n${text}`,
                },
              ],
            };
          }
        }

        return {
          content: [
            {
              type: "text",
              text: `0 matches. Try find_files to discover file/symbol names first, then grep with exact terms.`,
            },
          ],
        };
      }

      if (allItems.length === 0) {
        return {
          content: [{ type: "text", text: `0 matches.` }],
        };
      }

      const text = formatGrepResults(result.value, outputMode, maxResults);

      return {
        content: [{ type: "text", text }],
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
