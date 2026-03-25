import { executeHealth } from "./commands/health.js";
import { CursorStore } from "./cursor-store.js";
import * as finderManager from "./finder-manager.js";
import type { AutocompleteItem, ExtensionAPI } from "./pi-types.js";
import { FFF_GUIDELINES } from "./prompt-guidelines.js";
import { executeFindFiles } from "./tools/find-files.js";
import { executeGrep } from "./tools/grep.js";
import { executeMultiGrep } from "./tools/multi-grep.js";

const cursorStore = new CursorStore();

function getFinderOrThrow(cwd: string) {
  const result = finderManager.getFinder(cwd);
  if (!result.ok) throw new Error(`FFF init failed: ${result.error}`);
  return result.value;
}

/** Shared completion helper — fuzzy search files and return autocomplete items. */
function getFileCompletions(
  cwd: string,
  prefix: string,
): AutocompleteItem[] | null {
  const result = finderManager.getFinder(cwd);
  if (!result.ok) return null;

  const search = result.value.fileSearch(prefix, { pageSize: 15 });
  if (!search.ok || search.value.items.length === 0) return null;

  return search.value.items.map((item) => ({
    value: item.relativePath,
    label: item.relativePath,
  }));
}

export default function fffExtension(pi: ExtensionAPI) {
  // --- Tools ---

  pi.registerTool({
    name: "fff_find_files",
    label: "FFF Find Files",
    description:
      "Fuzzy file search by name. Searches FILE NAMES, not file contents. " +
      "Use fff_grep for searching code content. " +
      "Supports fuzzy matching, path prefixes ('src/'), and glob constraints ('*.{ts,tsx} !test/'). " +
      "IMPORTANT: Keep queries SHORT — prefer 1-2 terms max.",
    promptSnippet: "Fuzzy file search by name (fff)",
    promptGuidelines: [FFF_GUIDELINES],
    parameters: {
      type: "object",
      properties: {
        query: {
          type: "string",
          description:
            "Fuzzy search query. Supports path prefixes and glob constraints.",
        },
        maxResults: {
          type: "number",
          description: "Max results (default 20).",
        },
        cursor: {
          type: "string",
          description: "Cursor from previous result for pagination.",
        },
      },
      required: ["query"],
    },
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      const finder = getFinderOrThrow(ctx.cwd);
      await finder.waitForScan(10000);

      const text = executeFindFiles(
        finder,
        params.query,
        params.maxResults ?? 20,
        params.cursor,
        cursorStore,
      );

      return { content: [{ type: "text", text }] };
    },
  });

  pi.registerTool({
    name: "fff_grep",
    label: "FFF Grep",
    description:
      "Search file contents. Search for bare identifiers (e.g. 'InProgressQuote', 'ActorAuth'), " +
      "NOT code syntax or regex. Filter files with constraints (e.g. '*.rs query', 'src/ query'). " +
      "Use filename, directory (ending with /) or glob expressions to prefilter.",
    promptSnippet: "Search file contents (fff)",
    promptGuidelines: [FFF_GUIDELINES],
    parameters: {
      type: "object",
      properties: {
        query: {
          type: "string",
          description:
            "Search text or regex query with optional constraint prefixes. Matches within single lines only.",
        },
        maxResults: {
          type: "number",
          description: "Max matching lines (default 20).",
        },
        cursor: {
          type: "string",
          description: "Cursor from previous result for pagination.",
        },
        output_mode: {
          type: "string",
          description:
            "Output format: 'content' (default), 'files_with_matches', 'count', 'usage'.",
        },
      },
      required: ["query"],
    },
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      const finder = getFinderOrThrow(ctx.cwd);
      await finder.waitForScan(10000);

      const text = executeGrep(
        finder,
        params.query,
        params.maxResults ?? 20,
        params.cursor,
        params.output_mode,
        cursorStore,
      );

      return { content: [{ type: "text", text }] };
    },
  });

  pi.registerTool({
    name: "fff_multi_grep",
    label: "FFF Multi Grep",
    description:
      "Search file contents for lines matching ANY of multiple patterns (OR logic). " +
      "IMPORTANT: Returns files where ANY query matches, NOT all patterns. " +
      "Patterns are literal text — NEVER escape special characters. " +
      "Faster than regex alternation for literal text.",
    promptSnippet: "Multi-pattern content search (fff)",
    promptGuidelines: [FFF_GUIDELINES],
    parameters: {
      type: "object",
      properties: {
        patterns: {
          type: "array",
          items: { type: "string" },
          description:
            "Patterns to match (OR logic). Include all naming conventions: snake_case, PascalCase, camelCase.",
        },
        constraints: {
          type: "string",
          description:
            "File constraints (e.g. '*.{ts,tsx} !test/'). ALWAYS provide when possible.",
        },
        maxResults: {
          type: "number",
          description: "Max matching lines (default 20).",
        },
        cursor: {
          type: "string",
          description: "Cursor from previous result for pagination.",
        },
        output_mode: {
          type: "string",
          description:
            "Output format: 'content' (default), 'files_with_matches', 'count', 'usage'.",
        },
      },
      required: ["patterns"],
    },
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      const finder = getFinderOrThrow(ctx.cwd);
      await finder.waitForScan(10000);

      const text = executeMultiGrep(
        finder,
        params.patterns,
        params.constraints,
        params.maxResults ?? 20,
        params.cursor,
        params.output_mode,
        cursorStore,
      );

      return { content: [{ type: "text", text }] };
    },
  });

  // --- Commands with inline autocomplete ---

  // Store cwd from last tool call for completions (completions don't receive ctx)
  let lastCwd = process.cwd();

  pi.registerCommand("fff", {
    description: "Fuzzy find a file — type to search, select to mention",
    getArgumentCompletions(prefix: string) {
      return getFileCompletions(lastCwd, prefix);
    },
    async handler(args, ctx) {
      lastCwd = ctx.cwd;
      if (!args.trim()) {
        ctx.ui.notify("Usage: /fff <query> — fuzzy search files", "info");
        return;
      }
      const finder = getFinderOrThrow(ctx.cwd);
      await finder.waitForScan(5000);
      const text = executeFindFiles(finder, args.trim(), 10, undefined, cursorStore);
      ctx.ui.notify(text, "info");
    },
  });

  pi.registerCommand("fff-grep", {
    description: "Search file contents — type a pattern to search",
    async handler(args, ctx) {
      lastCwd = ctx.cwd;
      if (!args.trim()) {
        ctx.ui.notify("Usage: /fff-grep <query> — search file contents", "info");
        return;
      }
      const finder = getFinderOrThrow(ctx.cwd);
      await finder.waitForScan(5000);
      const text = executeGrep(finder, args.trim(), 10, undefined, undefined, cursorStore);
      ctx.ui.notify(text, "info");
    },
  });

  pi.registerCommand("fff-health", {
    description: "Show FFF file finder diagnostics",
    async handler(_args, ctx) {
      lastCwd = ctx.cwd;
      const finder = finderManager.getFinder(ctx.cwd);
      const text = executeHealth(finder.ok ? finder.value : null);
      ctx.ui.notify(text, "info");
    },
  });

  // --- Lifecycle ---

  pi.on("session_start", (_event: any, ctx: any) => {
    lastCwd = ctx.cwd;
    // Eagerly initialize the finder so files are indexed before first search
    finderManager.getFinder(ctx.cwd);
  });

  pi.on("session_shutdown", () => {
    finderManager.destroy();
  });
}
