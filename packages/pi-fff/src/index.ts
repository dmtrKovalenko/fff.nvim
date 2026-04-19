/**
 * pi-fff: FFF-powered file search extension for pi
 *
 * Overrides built-in `find` and `grep` tools with FFF and can also replace
 * @-mention autocomplete suggestions in the interactive editor.
 */

import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";
import {
  CustomEditor,
  truncateHead,
  DEFAULT_MAX_BYTES,
  formatSize,
} from "@mariozechner/pi-coding-agent";
import {
  Text,
  type AutocompleteItem,
  type AutocompleteProvider,
} from "@mariozechner/pi-tui";
import { Type } from "@sinclair/typebox";
import { FileFinder } from "@ff-labs/fff-node";
import type {
  GrepCursor,
  GrepMode,
  GrepResult,
  SearchResult,
  MixedItem,
} from "@ff-labs/fff-node";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_GREP_LIMIT = 100;
const DEFAULT_FIND_LIMIT = 200;
const GREP_MAX_LINE_LENGTH = 500;
const MENTION_MAX_RESULTS = 20;

type FffMode = "tools-and-ui" | "tools-only" | "override";

const VALID_MODES: FffMode[] = ["tools-and-ui", "tools-only", "override"];

interface ToolNames {
  grep: string;
  find: string;
  multiGrep: string;
}

const FFF_TOOL_NAMES: ToolNames = {
  grep: "ffgrep",
  find: "fffind",
  multiGrep: "fff-multi-grep",
};
const OVERRIDE_TOOL_NAMES: ToolNames = {
  grep: "grep",
  find: "find",
  multiGrep: "multi_grep",
};

function resolveToolNames(mode: FffMode): ToolNames {
  return mode === "override" ? OVERRIDE_TOOL_NAMES : FFF_TOOL_NAMES;
}

// ---------------------------------------------------------------------------
// Cursor store — simple bounded Map for pagination cursors
// ---------------------------------------------------------------------------

const cursorCache = new Map<string, GrepCursor>();
let cursorCounter = 0;

function storeCursor(cursor: GrepCursor): string {
  const id = `fff_c${++cursorCounter}`;
  cursorCache.set(id, cursor);
  if (cursorCache.size > 200) {
    const first = cursorCache.keys().next().value;
    if (first) cursorCache.delete(first);
  }
  return id;
}

function getCursor(id: string): GrepCursor | undefined {
  return cursorCache.get(id);
}

// ---------------------------------------------------------------------------
// Output formatting helpers
// ---------------------------------------------------------------------------

function truncateLine(line: string, max = GREP_MAX_LINE_LENGTH): string {
  const trimmed = line.trim();
  return trimmed.length <= max ? trimmed : `${trimmed.slice(0, max)}...`;
}

function formatGrepOutput(result: GrepResult, limit: number): string {
  const items = result.items.slice(0, limit);
  if (items.length === 0) return "No matches found";

  const lines: string[] = [];
  let currentFile = "";

  for (const match of items) {
    if (match.relativePath != currentFile) {
      currentFile = match.relativePath;
      if (lines.length > 0) lines.push("");
    }

    match.contextBefore?.forEach((line: string, i: number) => {
      lines.push(
        `${match.relativePath}-${match.lineNumber - match.contextBefore!.length + i}- ${truncateLine(line)}`,
      );
    });

    lines.push(
      `${match.relativePath}:${match.lineNumber}: ${truncateLine(match.lineContent)}`,
    );

    match.contextAfter?.forEach((line: string, i: number) => {
      lines.push(
        `${match.relativePath}-${match.lineNumber + 1 + i}- ${truncateLine(line)}`,
      );
    });
  }

  return lines.join("\n");
}

function formatFindOutput(result: SearchResult, limit: number): string {
  const items = result.items.slice(0, limit);
  return items.length === 0
    ? "No files found matching pattern"
    : items.map((i: { relativePath: string }) => i.relativePath).join("\n");
}

// ---------------------------------------------------------------------------
// Mention autocomplete helpers
// ---------------------------------------------------------------------------

function extractAtPrefix(textBeforeCursor: string): string | null {
  const match = textBeforeCursor.match(/(?:^|[ \t])(@(?:"[^"]*|[^\s]*))$/);
  return match?.[1] ?? null;
}

function buildAtCompletionValue(path: string): string {
  return path.includes(" ") ? `@"${path}"` : `@${path}`;
}

function createFffMentionProvider(
  getItems: (query: string, signal: AbortSignal) => Promise<AutocompleteItem[]>,
): AutocompleteProvider {
  return {
    async getSuggestions(lines, cursorLine, cursorCol, options) {
      const currentLine = lines[cursorLine] || "";
      const prefix = extractAtPrefix(currentLine.slice(0, cursorCol));
      if (!prefix || options.signal.aborted) return null;

      const query = prefix.startsWith('@"') ? prefix.slice(2) : prefix.slice(1);
      const items = await getItems(query, options.signal);
      return options.signal.aborted || items.length === 0 ? null : { items, prefix };
    },
    applyCompletion(_lines, cursorLine, cursorCol, item, prefix) {
      const currentLine = _lines[cursorLine] || "";
      const before = currentLine.slice(0, cursorCol - prefix.length);
      const after = currentLine.slice(cursorCol);
      const newLine = before + item.value + after;
      const newCursorCol = cursorCol - prefix.length + item.value.length;
      return {
        lines: [..._lines.slice(0, cursorLine), newLine, ..._lines.slice(cursorLine + 1)],
        cursorLine,
        cursorCol: newCursorCol,
      };
    },
  };
}

// Simple editor wrapper that injects FFF @-mention autocomplete alongside base provider
class FffEditor extends CustomEditor {
  private baseProvider: AutocompleteProvider | undefined;
  private getMentionItems: (
    query: string,
    signal: AbortSignal,
  ) => Promise<AutocompleteItem[]>;

  constructor(
    tui: any,
    theme: any,
    keybindings: any,
    getMentionItems: (query: string, signal: AbortSignal) => Promise<AutocompleteItem[]>,
  ) {
    super(tui, theme, keybindings);
    this.getMentionItems = getMentionItems;
  }

  override setAutocompleteProvider(provider: AutocompleteProvider): void {
    this.baseProvider = provider;
    // Create composite provider that handles @-mentions and falls back to base
    const mentionProvider = createFffMentionProvider(this.getMentionItems);
    const compositeProvider: AutocompleteProvider = {
      getSuggestions: async (lines, cursorLine, cursorCol, options) => {
        // Try @-mention first
        const mentionResult = await mentionProvider.getSuggestions(
          lines,
          cursorLine,
          cursorCol,
          options,
        );
        if (mentionResult) return mentionResult;
        // Fall back to base provider
        return (
          this.baseProvider?.getSuggestions(lines, cursorLine, cursorCol, options) ?? null
        );
      },
      applyCompletion: (lines, cursorLine, cursorCol, item, prefix) => {
        // Let mention provider handle @ completions, base provider for others
        if (prefix?.startsWith("@")) {
          return mentionProvider.applyCompletion!(
            lines,
            cursorLine,
            cursorCol,
            item,
            prefix,
          );
        }
        return (
          this.baseProvider?.applyCompletion?.(
            lines,
            cursorLine,
            cursorCol,
            item,
            prefix,
          ) ?? { lines, cursorLine, cursorCol }
        );
      },
    };
    super.setAutocompleteProvider(compositeProvider);
  }
}

// ---------------------------------------------------------------------------
// Extension
// ---------------------------------------------------------------------------

export default function fffExtension(pi: ExtensionAPI) {
  let finder: FileFinder | null = null;
  let finderCwd: string | null = null;
  let activeCwd = process.cwd();

  // Mode resolution: flag > env > default
  let currentMode: FffMode =
    (pi.getFlag("fff-mode") as FffMode) ??
    (process.env.PI_FFF_MODE as FffMode) ??
    "tools-and-ui";

  const toolNames = resolveToolNames(currentMode);

  // DB path resolution: flag > env > undefined (use fff-node defaults)
  const frecencyDbPath =
    (pi.getFlag("fff-frecency-db") as string | undefined) ??
    process.env.FFF_FRECENCY_DB ??
    undefined;
  const historyDbPath =
    (pi.getFlag("fff-history-db") as string | undefined) ??
    process.env.FFF_HISTORY_DB ??
    undefined;

  function getMode(): FffMode {
    return currentMode;
  }

  function setMode(mode: FffMode): void {
    currentMode = mode;
  }

  function shouldEnableMentions(): boolean {
    return currentMode !== "tools-only";
  }

  async function ensureFinder(cwd: string): Promise<FileFinder> {
    if (finder && !finder.isDestroyed && finderCwd === cwd) return finder;
    if (finder && !finder.isDestroyed) {
      finder.destroy();
      finder = null;
      finderCwd = null;
    }

    const result = FileFinder.create({
      basePath: cwd,
      frecencyDbPath,
      historyDbPath,
      aiMode: true,
    });

    if (!result.ok) throw new Error(`Failed to create FFF file finder: ${result.error}`);

    finder = result.value;
    finderCwd = cwd;
    await finder.waitForScan(15000);
    return finder;
  }

  function destroyFinder() {
    if (finder && !finder.isDestroyed) {
      finder.destroy();
      finder = null;
      finderCwd = null;
    }
  }

  async function getMentionItems(
    query: string,
    signal: AbortSignal,
  ): Promise<AutocompleteItem[]> {
    if (signal.aborted) return [];
    const f = await ensureFinder(activeCwd);
    if (signal.aborted) return [];

    const result = f.mixedSearch(query, { pageSize: MENTION_MAX_RESULTS });
    if (!result.ok) return [];

    return result.value.items.slice(0, MENTION_MAX_RESULTS).map((mixed: MixedItem) => {
      if (mixed.type === "directory") {
        return {
          value: buildAtCompletionValue(mixed.item.relativePath),
          label: mixed.item.dirName,
          description: mixed.item.relativePath,
        };
      }
      return {
        value: buildAtCompletionValue(mixed.item.relativePath),
        label: mixed.item.fileName,
        description: mixed.item.relativePath,
      };
    });
  }

  function applyEditorMode(ctx: {
    ui: {
      setEditorComponent: (
        factory: ((tui: any, theme: any, keybindings: any) => any) | undefined,
      ) => void;
    };
  }) {
    if (!shouldEnableMentions()) {
      ctx.ui.setEditorComponent(undefined);
    } else {
      ctx.ui.setEditorComponent(
        (tui: any, theme: any, keybindings: any) =>
          new FffEditor(tui, theme, keybindings, getMentionItems),
      );
    }
  }

  // --- Flags / lifecycle ---

  pi.registerFlag("fff-mode", {
    description: "FFF mode: tools-and-ui | tools-only | override",
    type: "string",
  });

  pi.registerFlag("fff-frecency-db", {
    description: "Path to the frecency database (overrides FFF_FRECENCY_DB env)",
    type: "string",
  });

  pi.registerFlag("fff-history-db", {
    description: "Path to the query history database (overrides FFF_HISTORY_DB env)",
    type: "string",
  });

  pi.on("session_start", async (_event, ctx) => {
    try {
      activeCwd = ctx.cwd;
      await ensureFinder(activeCwd);
      applyEditorMode(ctx);
    } catch (e: unknown) {
      ctx.ui.notify(
        `FFF init failed: ${e instanceof Error ? e.message : String(e)}`,
        "error",
      );
    }
  });

  pi.on("session_shutdown", async () => {
    destroyFinder();
  });

  // --- Shared render helpers ---

  const renderTextResult = (
    result: { content?: { type: string; text?: string }[] },
    options: { expanded?: boolean },
    theme: any,
    context: any,
    maxLines = 15,
  ) => {
    const text = (context.lastComponent as Text | undefined) ?? new Text("", 0, 0);
    const output = result.content?.find((c) => c.type === "text")?.text?.trim() ?? "";
    if (!output) {
      text.setText(theme.fg("muted", "No output"));
      return text;
    }

    const lines = output.split("\n");
    const displayLines = lines.slice(0, options.expanded ? lines.length : maxLines);
    let content = `\n${displayLines.map((line: string) => theme.fg("toolOutput", line)).join("\n")}`;
    if (lines.length > displayLines.length) {
      content += theme.fg(
        "muted",
        `\n... (${lines.length - displayLines.length} more lines)`,
      );
    }
    text.setText(content);
    return text;
  };

  // --- grep tool ---

  const grepSchema = Type.Object({
    pattern: Type.String({ description: "Search pattern (plain text or regex)" }),
    path: Type.Optional(
      Type.String({
        description:
          "Directory or file constraint, e.g. 'src/' or '*.ts' (default: project root)",
      }),
    ),
    literal: Type.Optional(
      Type.Boolean({
        description: "Treat pattern as literal string instead of regex (default: true)",
      }),
    ),
    context: Type.Optional(
      Type.Number({
        description: "Number of lines to show before and after each match (default: 0)",
      }),
    ),
    limit: Type.Optional(
      Type.Number({
        description: `Maximum number of matches to return (default: ${DEFAULT_GREP_LIMIT})`,
      }),
    ),
    cursor: Type.Optional(
      Type.String({ description: "Cursor from previous result for pagination" }),
    ),
  });

  pi.registerTool({
    name: toolNames.grep,
    label: toolNames.grep,
    description: `Search file contents for a pattern using FFF (fast, frecency-ranked, git-aware). Returns matching lines with file paths and line numbers. Respects .gitignore. Supports plain text, regex, and fuzzy search modes. Smart case by default. Output truncated to ${DEFAULT_GREP_LIMIT} matches or ${DEFAULT_MAX_BYTES / 1024}KB.`,
    promptSnippet:
      "Search file contents for patterns (FFF: frecency-ranked, git-aware, respects .gitignore)",
    promptGuidelines: [
      "Search for bare identifiers (e.g. 'InProgressQuote'), not code syntax or multi-token regex.",
      "Plain text search is faster and more reliable than regex. Prefer it.",
      "After 2 grep calls, read the top result file instead of grepping more.",
      "Use the path parameter for file/directory constraints: '*.ts', 'src/'.",
    ],
    parameters: grepSchema,

    async execute(_toolCallId, params, signal) {
      if (signal?.aborted) throw new Error("Operation aborted");

      const f = await ensureFinder(activeCwd);
      const effectiveLimit = Math.max(1, params.limit ?? DEFAULT_GREP_LIMIT);
      const query = params.path ? `${params.path} ${params.pattern}` : params.pattern;
      const mode: GrepMode = params.literal === false ? "regex" : "plain";

      const grepResult = f.grep(query, {
        mode,
        smartCase: true,
        maxMatchesPerFile: Math.min(effectiveLimit, 50),
        cursor: (params.cursor ? getCursor(params.cursor) : null) ?? null,
        beforeContext: params.context ?? 0,
        afterContext: params.context ?? 0,
      });

      if (!grepResult.ok) throw new Error(grepResult.error);

      const result = grepResult.value;
      let output = formatGrepOutput(result, effectiveLimit);
      const truncation = truncateHead(output, { maxLines: Number.MAX_SAFE_INTEGER });
      output = truncation.content;

      const notices: string[] = [];
      if (result.items.length >= effectiveLimit)
        notices.push(
          `${effectiveLimit} matches limit reached. Use limit=${effectiveLimit * 2} for more`,
        );
      if (truncation.truncated)
        notices.push(`${formatSize(DEFAULT_MAX_BYTES)} limit reached`);
      if (result.regexFallbackError)
        notices.push(`Regex failed: ${result.regexFallbackError}, used literal match`);
      if (result.nextCursor)
        notices.push(
          `More results available. Use cursor="${storeCursor(result.nextCursor)}" to continue`,
        );

      if (notices.length > 0) output += `\n\n[${notices.join(". ")}]`;

      return {
        content: [{ type: "text", text: output }],
        details: {
          totalMatched: result.totalMatched,
          totalFiles: result.totalFiles,
          truncated: truncation.truncated,
        },
      };
    },

    renderCall(args, theme, context) {
      const text = (context.lastComponent as Text | undefined) ?? new Text("", 0, 0);
      const pattern = args?.pattern ?? "";
      const path = args?.path ?? ".";
      let content =
        theme.fg("toolTitle", theme.bold(toolNames.grep)) +
        " " +
        theme.fg("accent", `/${pattern}/`) +
        theme.fg("toolOutput", ` in ${path}`);
      if (args?.limit !== undefined)
        content += theme.fg("toolOutput", ` limit ${args.limit}`);
      if (args?.cursor) content += theme.fg("muted", ` (page)`);
      text.setText(content);
      return text;
    },

    renderResult(result, options, theme, context) {
      return renderTextResult(result, options, theme, context, 15);
    },
  });

  // --- find tool ---

  const findSchema = Type.Object({
    pattern: Type.String({
      description:
        "Fuzzy search query for file names. Supports path prefixes ('src/') and globs ('*.ts').",
    }),
    path: Type.Optional(
      Type.String({ description: "Directory to search in (default: project root)" }),
    ),
    limit: Type.Optional(
      Type.Number({
        description: `Maximum number of results (default: ${DEFAULT_FIND_LIMIT})`,
      }),
    ),
  });

  pi.registerTool({
    name: toolNames.find,
    label: toolNames.find,
    description: `Fuzzy file search by name using FFF (fast, frecency-ranked, git-aware). Returns matching file paths relative to project root. Respects .gitignore. Supports fuzzy matching, path prefixes ('src/'), and glob constraints ('*.ts', '**/*.spec.ts'). Output truncated to ${DEFAULT_FIND_LIMIT} results or ${DEFAULT_MAX_BYTES / 1024}KB.`,
    promptSnippet:
      "Find files by name (FFF: fuzzy, frecency-ranked, git-aware, respects .gitignore)",
    promptGuidelines: [
      "Keep queries short -- prefer 1-2 terms max.",
      "Multiple words narrow results (waterfall), they are not OR.",
      "Use this to find files by name. Use grep to search file contents.",
    ],
    parameters: findSchema,

    async execute(_toolCallId, params, signal) {
      if (signal?.aborted) throw new Error("Operation aborted");

      const f = await ensureFinder(activeCwd);
      const effectiveLimit = Math.max(1, params.limit ?? DEFAULT_FIND_LIMIT);
      const query = params.path ? `${params.path} ${params.pattern}` : params.pattern;

      const searchResult = f.fileSearch(query, { pageSize: effectiveLimit });
      if (!searchResult.ok) throw new Error(searchResult.error);

      const result = searchResult.value;
      let output = formatFindOutput(result, effectiveLimit);
      const truncation = truncateHead(output, { maxLines: Number.MAX_SAFE_INTEGER });
      output = truncation.content;

      const notices: string[] = [];
      if (result.items.length >= effectiveLimit)
        notices.push(
          `${effectiveLimit} results limit reached. Use limit=${effectiveLimit * 2} for more, or refine pattern`,
        );
      if (truncation.truncated)
        notices.push(`${formatSize(DEFAULT_MAX_BYTES)} limit reached`);
      if (result.totalMatched > result.items.length)
        notices.push(
          `${result.totalMatched} total matches (${result.totalFiles} indexed files)`,
        );

      if (notices.length > 0) output += `\n\n[${notices.join(". ")}]`;

      return {
        content: [{ type: "text", text: output }],
        details: {
          totalMatched: result.totalMatched,
          totalFiles: result.totalFiles,
          truncated: truncation.truncated,
        },
      };
    },

    renderCall(args, theme, context) {
      const text = (context.lastComponent as Text | undefined) ?? new Text("", 0, 0);
      const pattern = args?.pattern ?? "";
      const path = args?.path ?? ".";
      let content =
        theme.fg("toolTitle", theme.bold(toolNames.find)) +
        " " +
        theme.fg("accent", pattern) +
        theme.fg("toolOutput", ` in ${path}`);
      if (args?.limit !== undefined)
        content += theme.fg("toolOutput", ` (limit ${args.limit})`);
      text.setText(content);
      return text;
    },

    renderResult(result, options, theme, context) {
      return renderTextResult(result, options, theme, context, 20);
    },
  });

  // --- multi_grep tool ---

  const multiGrepSchema = Type.Object({
    patterns: Type.Array(Type.String(), {
      description:
        "Patterns to search for (OR logic -- matches lines containing ANY pattern). Include all naming conventions: snake_case, PascalCase, camelCase.",
    }),
    constraints: Type.Optional(
      Type.String({
        description:
          "File constraints, e.g. '*.{ts,tsx} !test/' to filter files. Separate from patterns.",
      }),
    ),
    context: Type.Optional(
      Type.Number({
        description: "Number of context lines before and after each match (default: 0)",
      }),
    ),
    limit: Type.Optional(
      Type.Number({
        description: `Maximum number of matches to return (default: ${DEFAULT_GREP_LIMIT})`,
      }),
    ),
    cursor: Type.Optional(
      Type.String({ description: "Cursor from previous result for pagination" }),
    ),
  });

  pi.registerTool({
    name: toolNames.multiGrep,
    label: toolNames.multiGrep,
    description:
      "Search file contents for lines matching ANY of multiple patterns (OR logic). Uses SIMD-accelerated Aho-Corasick multi-pattern matching. Faster than regex alternation. Patterns are literal text -- never escape special characters. Use the constraints parameter for file filtering ('*.rs', 'src/', '!test/').",
    promptSnippet:
      "Multi-pattern OR search across file contents (FFF: SIMD-accelerated, frecency-ranked)",
    promptGuidelines: [
      `Use ${toolNames.multiGrep} when you need to find multiple identifiers at once (OR logic).`,
      "Include all naming conventions: snake_case, PascalCase, camelCase variants.",
      "Patterns are literal text. Never escape special characters.",
      "Use the constraints parameter for file type/path filtering, not inside patterns.",
    ],
    parameters: multiGrepSchema,

    async execute(_toolCallId, params, signal) {
      if (signal?.aborted) throw new Error("Operation aborted");
      if (!params.patterns?.length)
        throw new Error("patterns array must have at least 1 element");

      const f = await ensureFinder(activeCwd);
      const effectiveLimit = Math.max(1, params.limit ?? DEFAULT_GREP_LIMIT);

      const grepResult = f.multiGrep({
        patterns: params.patterns,
        constraints: params.constraints,
        maxMatchesPerFile: Math.min(effectiveLimit, 50),
        smartCase: true,
        cursor: (params.cursor ? getCursor(params.cursor) : null) ?? null,
        beforeContext: params.context ?? 0,
        afterContext: params.context ?? 0,
      });

      if (!grepResult.ok) throw new Error(grepResult.error);

      const result = grepResult.value;
      let output = formatGrepOutput(result, effectiveLimit);
      const truncation = truncateHead(output, { maxLines: Number.MAX_SAFE_INTEGER });
      output = truncation.content;

      const notices: string[] = [];
      if (result.items.length >= effectiveLimit)
        notices.push(
          `${effectiveLimit} matches limit reached. Use limit=${effectiveLimit * 2} for more`,
        );
      if (truncation.truncated)
        notices.push(`${formatSize(DEFAULT_MAX_BYTES)} limit reached`);
      if (result.nextCursor)
        notices.push(
          `More results available. Use cursor="${storeCursor(result.nextCursor)}" to continue`,
        );

      if (notices.length > 0) output += `\n\n[${notices.join(". ")}]`;

      return {
        content: [{ type: "text", text: output }],
        details: {
          totalMatched: result.totalMatched,
          totalFiles: result.totalFiles,
          truncated: truncation.truncated,
          patterns: params.patterns,
        },
      };
    },

    renderCall(args, theme, context) {
      const text = (context.lastComponent as Text | undefined) ?? new Text("", 0, 0);
      const patterns = args?.patterns ?? [];
      const constraints = args?.constraints;
      let content =
        theme.fg("toolTitle", theme.bold(toolNames.multiGrep)) +
        " " +
        theme.fg("accent", patterns.map((p: string) => `"${p}"`).join(", "));
      if (constraints) content += theme.fg("toolOutput", ` (${constraints})`);
      if (args?.cursor) content += theme.fg("muted", ` (page)`);
      text.setText(content);
      return text;
    },

    renderResult(result, options, theme, context) {
      return renderTextResult(result, options, theme, context, 15);
    },
  });

  // --- commands ---

  pi.registerCommand("fff-mode", {
    description: "Show or set FFF mode: /fff-mode [tools-and-ui | tools-only | override]",
    handler: async (args, ctx) => {
      const arg = (args || "").trim();

      // No args - show current mode
      if (!arg) {
        const mode = getMode();
        const flag = pi.getFlag("fff-mode") ?? "unset";
        const env = process.env.PI_FFF_MODE ?? "unset";
        ctx.ui.notify(`Current mode: '${mode}'\nFlag: ${flag}, Env: ${env}`, "info");
        return;
      }

      // Validate and set mode
      if (!VALID_MODES.includes(arg as FffMode)) {
        ctx.ui.notify(`Usage: /fff-mode [${VALID_MODES.join(" | ")}]`, "warning");
        return;
      }

      const newMode = arg as FffMode;
      const oldMode = getMode();
      setMode(newMode);

      // Apply immediately using the shared function
      applyEditorMode(ctx);

      const note =
        (oldMode === "override") !== (newMode === "override")
          ? " (tool name change requires restart)"
          : "";
      ctx.ui.notify(`Mode changed: '${oldMode}' → '${newMode}'${note}`, "info");
    },
  });

  pi.registerCommand("fff-health", {
    description: "Show FFF file finder health and status",
    handler: async (_args, ctx) => {
      if (!finder || finder.isDestroyed) {
        ctx.ui.notify("FFF not initialized", "warning");
        return;
      }

      const health = finder.healthCheck();
      if (!health.ok) {
        ctx.ui.notify(`Health check failed: ${health.error}`, "error");
        return;
      }

      const h = health.value;
      const lines = [
        `FFF v${h.version}`,
        `Mode: ${getMode()}`,
        `Git: ${h.git.repositoryFound ? `yes (${h.git.workdir ?? "unknown"})` : "no"}`,
        `Picker: ${h.filePicker.initialized ? `${h.filePicker.indexedFiles ?? 0} files` : "not initialized"}`,
        `Frecency: ${h.frecency.initialized ? "active" : "disabled"}`,
        `Query tracker: ${h.queryTracker.initialized ? "active" : "disabled"}`,
      ];

      const progress = finder.getScanProgress();
      if (progress.ok) {
        lines.push(
          `Scanning: ${progress.value.isScanning ? "yes" : "no"} (${progress.value.scannedFilesCount} files)`,
        );
      }

      ctx.ui.notify(lines.join("\n"), "info");
    },
  });

  pi.registerCommand("fff-rescan", {
    description: "Trigger FFF to rescan files",
    handler: async (_args, ctx) => {
      if (!finder || finder.isDestroyed) {
        ctx.ui.notify("FFF not initialized", "warning");
        return;
      }

      const result = finder.scanFiles();
      if (!result.ok) {
        ctx.ui.notify(`Rescan failed: ${result.error}`, "error");
        return;
      }

      ctx.ui.notify("FFF rescan triggered", "info");
    },
  });
}
