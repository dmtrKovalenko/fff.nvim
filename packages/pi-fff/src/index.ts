/**
 * pi-fff: FFF-powered file search extension for pi
 *
 * Overrides built-in `find` and `grep` tools with FFF and adds FFF-backed
 * @-mention autocomplete suggestions to the interactive editor.
 */

import nodePath from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import {
  type AutocompleteItem,
  type AutocompleteProvider,
  Text,
} from "@earendil-works/pi-tui";
import type {
  FileFinderApi,
  GrepCursor,
  GrepMode,
  GrepResult,
  MixedItem,
  SearchResult,
} from "@ff-labs/fff-node";
import { Type } from "@sinclair/typebox";
import { AuxFinderPool, routePathConstraint } from "./aux-finders";
import { buildQuery } from "./query";
import { isHomeDir } from "./paths";
import { loadSdk, SCAN_TIMEOUT_MS } from "./sdk";

export { SCAN_TIMEOUT_MS } from "./sdk";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_GREP_LIMIT = 20;
const DEFAULT_FIND_LIMIT = 30;
const GREP_PAGE_SIZE_MAX = 50;
const GREP_CONTEXT_MAX = 20;
const GREP_MAX_LINE_LENGTH = 500;
const MENTION_MAX_RESULTS = 20;

// If we exceed 10 seconds for indexed grep - something is definitely off
const GREP_TIME_BUDGET_MS = 10_000;

const HOME_SCAN_STATUS_KEY = "fff";
const HOME_SCAN_POLL_MS = 1_000;
const HOME_SCAN_DISABLE_HINT =
  "You can prevent home dir indexing with --fff-enable-home-scan=false (or FFF_ENABLE_HOME_SCAN=0).";

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

// Find pagination uses a page-index cursor: native `fileSearch` takes
// pageIndex/pageSize, so the cursor is just the next page index paired with
// the query+limit that produced it. Stored tokens are opaque IDs to the agent.
interface FindCursor {
  query: string;
  pattern: string;
  pageSize: number;
  nextPageIndex: number;
  auxRoot?: string;
}

const findCursorCache = new Map<string, FindCursor>();
let findCursorCounter = 0;

function storeFindCursor(cursor: FindCursor): string {
  const id = `${++findCursorCounter}`;
  findCursorCache.set(id, cursor);
  if (findCursorCache.size > 200) {
    const first = findCursorCache.keys().next().value;
    if (first) findCursorCache.delete(first);
  }
  return id;
}

function getFindCursor(id: string): FindCursor | undefined {
  return findCursorCache.get(id);
}

// ---------------------------------------------------------------------------
// Output formatting helpers
// ---------------------------------------------------------------------------

function truncateLine(line: string, max = GREP_MAX_LINE_LENGTH): string {
  const trimmed = line.trim();
  return trimmed.length <= max ? trimmed : `${trimmed.slice(0, max)}...`;
}

// Clamp caller-supplied context to a non-negative bounded integer so a large
// value cannot multiply output size past the model window.
function clampContext(context: number | undefined): number {
  if (!context || context < 0) return 0;
  return Math.min(Math.floor(context), GREP_CONTEXT_MAX);
}

const HOT_FRECENCY = 25;
const WARM_FRECENCY = 20;

// Shared annotation helper for both find-output paths and grep-output file
// headers. Returns at most ONE tag so output stays scannable. Priority:
// git-dirty (most actionable — file is changing right now) beats frecency
// (historically often-touched). Keeping one function ensures the two tools
// never drift in how they surface git/frecency signal.
export function fffFileAnnotation(item: {
  gitStatus?: string;
  totalFrecencyScore?: number;
  accessFrecencyScore?: number;
}): string {
  const git = item.gitStatus;
  if (git && git !== "clean" && git !== "unknown" && git !== "") {
    return `  [${git} in git]`;
  }

  const frecency = item.totalFrecencyScore ?? item.accessFrecencyScore ?? 0;
  if (frecency >= HOT_FRECENCY) return "  [VERY often touched file]";
  if (frecency >= WARM_FRECENCY) return "  [often touched file]";

  return "";
}

// fff-core native definition classifier (byte-level scanner in Rust) is enabled
// via GrepOptions.classifyDefinitions. Each GrepMatch carries isDefinition for
// downstream consumers; pi-fff does NOT use it to re-sort.
//
// Ordering policy: NO CUSTOM SORTING. The engine already returns items in
// frecency order (most-accessed files first). pi-fff only groups consecutive
// matches into per-file blocks and preserves whatever order the engine
// provided — inside a file we keep matches in source-line order because the
// engine emits them that way.

function formatGrepOutput(result: GrepResult): string {
  if (result.items.length === 0) return "No matches found";

  // Build file-grouped output in the order files first appear in the result.
  // This preserves native frecency ordering across files without re-sorting.
  const lines: string[] = [];
  let currentFile = "";
  let shown = 0;

  for (const match of result.items) {
    if (match.relativePath !== currentFile) {
      if (lines.length > 0) lines.push("");
      currentFile = match.relativePath;
      lines.push(`${currentFile}${fffFileAnnotation(match)}`);
    }

    match.contextBefore?.forEach((line: string, i: number) => {
      const lineNum = match.lineNumber - match.contextBefore!.length + i;
      lines.push(` ${lineNum}- ${truncateLine(line)}`);
    });

    lines.push(` ${match.lineNumber}: ${truncateLine(match.lineContent)}`);
    shown++;

    match.contextAfter?.forEach((line: string, i: number) => {
      const lineNum = match.lineNumber + 1 + i;
      lines.push(` ${lineNum}- ${truncateLine(line)}`);
    });
  }

  return lines.join("\n");
}

// Weak-match threshold is derived from the query length, matching the
// scoring formula in crates/fff-core/src/score.rs: a perfect match scores
// `len * 16`, so we treat anything below 50% of that as scattered fuzzy noise.
// When the top score is weak, trim output to a small sample instead of dumping
// the full limit worth of noise into the agent's context.
const FIND_WEAK_SAMPLE_SIZE = 5;

function weakScoreThreshold(pattern: string): number {
  const perfect = pattern.length * 12;
  return Math.floor((perfect * 50) / 100);
}

interface FormattedFind {
  output: string;
  weak: boolean;
  shownCount: number;
}

function formatFindOutput(
  result: SearchResult,
  limit: number,
  pattern: string,
): FormattedFind {
  if (result.items.length === 0) {
    return {
      output: "No files found matching pattern",
      weak: false,
      shownCount: 0,
    };
  }

  // NO CUSTOM SORTING — trust native frecency order from the engine.
  const reordered = result.items.map((item) => ({ item }));

  // Peek at the top native score to decide whether results are scattered
  // fuzzy noise (query length-scaled threshold from score.rs).
  const topScore = result.scores[0]?.total ?? 0;
  const weak = topScore < weakScoreThreshold(pattern);
  const effective = weak ? Math.min(FIND_WEAK_SAMPLE_SIZE, limit) : limit;
  const shown = reordered.slice(0, effective);

  return {
    output: shown
      .map((p) => `${p.item.relativePath}${fffFileAnnotation(p.item)}`)
      .join("\n"),
    weak,
    shownCount: shown.length,
  };
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

// ---------------------------------------------------------------------------
// Extension
// ---------------------------------------------------------------------------

export default function fffExtension(pi: ExtensionAPI) {
  let mainFinder: FileFinderApi | null = null;
  let finderCwd: string | null = null;
  // Concurrent ensureFinder() callers share the same in-flight promise so
  // FileFinder.create() (which takes native DB locks) runs at most once per
  // base path at a time — otherwise parallel tool calls would race and
  // deadlock at the native layer (issue #403).
  let finderPromise: Promise<FileFinderApi> | null = null;
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

  // flag (boolean) > env ("1"/"true", or "0"/"false") > default.
  function resolveBoolOpt(
    flagName: string,
    envName: string,
    fallback = false,
  ): boolean {
    const flag = pi.getFlag(flagName);
    if (typeof flag === "boolean") return flag;
    if (typeof flag === "string") return flag === "true" || flag === "1";
    const env = process.env[envName];
    if (env === "1" || env === "true") return true;
    if (env === "0" || env === "false") return false;
    return fallback;
  }
  // Root scanning opt-in: FFF refuses to init at / unless this is set.
  const enableFsRootScanning = resolveBoolOpt(
    "fff-enable-root-scan",
    "FFF_ENABLE_ROOT_SCAN",
  );
  // Home dir scanning is on by default (launching pi from $HOME is a normal
  // flow), but configurable so users with huge $HOME trees can opt out.
  const enableHomeDirScanning = resolveBoolOpt(
    "fff-enable-home-scan",
    "FFF_ENABLE_HOME_SCAN",
    true,
  );

  function getMode(): FffMode {
    return currentMode;
  }

  function setMode(mode: FffMode): void {
    currentMode = mode;
  }

  function shouldEnableMentions(): boolean {
    return currentMode !== "tools-only";
  }

  // Set on session_start; the only handle to the UI outside an event handler.
  // setStatus is TUI/RPC-only, hence optional.
  let uiCtx: {
    ui: {
      notify: (message: string, type?: "info" | "warning" | "error") => void;
      setStatus?: (key: string, text: string | undefined) => void;
    };
  } | null = null;
  let homeScanTimer: ReturnType<typeof setInterval> | null = null;

  function warnHomeDirScan(root: string): void {
    uiCtx?.ui.notify(
      `(fff): Your cwd (${root}) is too large. Indexing will take additional time and resources.\n${HOME_SCAN_DISABLE_HINT}`,
      "warning",
    );
  }

  let auxPool = new AuxFinderPool({
    enableFsRootScanning,
    enableHomeDirScanning,
    onHomeDirScan: warnHomeDirScan,
  });

  // in case cwd changes we need to figure this out
  function ensureFinder(cwd: string): Promise<FileFinderApi> {
    if (mainFinder && !mainFinder.isDestroyed && finderCwd === cwd)
      return Promise.resolve(mainFinder);

    if (finderPromise) return finderPromise;

    finderPromise = (async () => {
      if (mainFinder && !mainFinder.isDestroyed) {
        mainFinder.destroy();
        mainFinder = null;
        finderCwd = null;
      }

      const { FileFinder } = await loadSdk();
      const result = FileFinder.create({
        basePath: cwd,
        frecencyDbPath,
        historyDbPath,
        aiMode: true,
        enableHomeDirScanning,
        enableFsRootScanning,
      });

      if (!result.ok)
        throw new Error(`Failed to create FFF file finder: ${result.error}`);

      mainFinder = result.value;
      finderCwd = cwd;
      await mainFinder.waitForScan(SCAN_TIMEOUT_MS);
      return mainFinder;
    })().finally(() => {
      finderPromise = null;
    });

    return finderPromise;
  }

  function stopHomeScanStatus(): void {
    if (homeScanTimer) {
      clearInterval(homeScanTimer);
      homeScanTimer = null;
    }
    uiCtx?.ui.setStatus?.(HOME_SCAN_STATUS_KEY, undefined);
  }

  // waitForScan() resolves on timeout too, so the scan can still be running.
  // Poll the live progress until it settles, then clear the footer.
  function trackHomeScanStatus(): void {
    stopHomeScanStatus();
    if (!uiCtx?.ui.setStatus) return;

    const tick = () => {
      const progress = mainFinder?.getScanProgress?.();
      if (!progress?.ok || !progress.value.isScanning) {
        stopHomeScanStatus();
        return;
      }
      uiCtx?.ui.setStatus?.(
        HOME_SCAN_STATUS_KEY,
        `Agent is indexing $HOME (${progress.value.scannedFilesCount} files), this can lead to high CPU`,
      );
    };

    homeScanTimer = setInterval(tick, HOME_SCAN_POLL_MS);
    // Must not hold the process open once pi is done.
    (homeScanTimer as { unref?: () => void }).unref?.();
    tick();
  }

  function destroyFinder() {
    stopHomeScanStatus();
    if (mainFinder && !mainFinder.isDestroyed) {
      mainFinder.destroy();
      mainFinder = null;
      finderCwd = null;
    }

    if (auxPool) {
      auxPool.destroy();
    }
  }

  async function resolveFinderForPath(
    pathParam: string | undefined,
    pattern: string,
    exclude: string | string[] | undefined,
  ): Promise<{ finder: FileFinderApi; query: string; root: string } | null> {
    const route = routePathConstraint(pathParam, activeCwd);
    if (!route) return null;
    const aux = await auxPool.acquire(route.root);
    // A broader covering picker may have been reused; rebase the suffix so the
    // constraint stays relative to the picker's actual root.
    const rebase = nodePath.relative(aux.root, route.root).replaceAll(nodePath.sep, "/");
    const suffix = [rebase, route.suffix].filter(Boolean).join("/");
    const query = buildQuery(suffix || undefined, pattern, exclude, aux.root);
    return { finder: aux.finder, query, root: aux.root };
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

  function registerAutocompleteProvider(ctx: {
    ui: {
      addAutocompleteProvider?: (
        factory: (current: AutocompleteProvider) => AutocompleteProvider,
      ) => void;
    };
  }) {
    // pi forks (e.g. omp) may not expose addAutocompleteProvider; skip UI wiring
    // and let tools continue to work instead of failing session_start.
    if (typeof ctx.ui.addAutocompleteProvider !== "function") return;

    ctx.ui.addAutocompleteProvider((current) => {
      const mentionProvider = createFffMentionProvider(getMentionItems);

      return {
        async getSuggestions(lines, cursorLine, cursorCol, options) {
          if (shouldEnableMentions()) {
            try {
              const mentionResult = await mentionProvider.getSuggestions(
                lines,
                cursorLine,
                cursorCol,
                options,
              );
              if (mentionResult) return mentionResult;
            } catch {
              // Delegate when FFF lookup is unavailable.
            }
          }

          return current.getSuggestions(lines, cursorLine, cursorCol, options);
        },
        applyCompletion(lines, cursorLine, cursorCol, item, prefix) {
          return current.applyCompletion(lines, cursorLine, cursorCol, item, prefix);
        },
        shouldTriggerFileCompletion(lines, cursorLine, cursorCol) {
          return (
            current.shouldTriggerFileCompletion?.(lines, cursorLine, cursorCol) ?? true
          );
        },
      };
    });
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

  pi.registerFlag("fff-enable-root-scan", {
    description:
      "Allow indexing when launched from the filesystem root (also: FFF_ENABLE_ROOT_SCAN env)",
    type: "boolean",
  });

  pi.registerFlag("fff-enable-home-scan", {
    description:
      "Index the home dir when launched from $HOME (default true; disable with --fff-enable-home-scan=false or FFF_ENABLE_HOME_SCAN=0)",
    type: "boolean",
  });

  pi.on("session_start", async (_event, ctx) => {
    try {
      activeCwd = ctx.cwd;
      uiCtx = ctx as unknown as typeof uiCtx;

      // Restore persisted mode from session entries. This handles session
      // resume after process restart where env vars are lost, and ensures
      // the env var is set for the next /reload in the same session.
      const entries = ctx.sessionManager?.getEntries();
      if (entries) {
        const modeEntry = [...entries]
          .reverse()
          .find(
            (e: { type: string; customType?: string }) =>
              e.type === "custom" && e.customType === "fff-mode",
          );
        if (
          modeEntry &&
          typeof (modeEntry as any).data?.mode === "string" &&
          VALID_MODES.includes((modeEntry as any).data.mode as FffMode)
        ) {
          const restored = (modeEntry as any).data.mode as FffMode;
          if (restored !== currentMode) {
            currentMode = restored;
          }
        }
      }

      registerAutocompleteProvider(ctx);
      await ensureFinder(activeCwd);

      // Warn when launched from $HOME with home scanning on: indexing a large
      // home tree can run for a long time in the background (issue #743).
      const atHome = enableHomeDirScanning && isHomeDir(activeCwd);
      if (atHome) {
        warnHomeDirScan(activeCwd);
        ctx.ui.setStatus?.(
          HOME_SCAN_STATUS_KEY,
          "Agent is indexing $HOME, this can lead to high CPU",
        );
      }

      // waitForScan() also resolves on timeout, so poll until the scan really
      // settles before clearing the footer.
      if (atHome) trackHomeScanStatus();
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
    pattern: Type.String({
      description: "Search pattern (literal text or regex)",
    }),
    path: Type.Optional(
      Type.String({
        description:
          "Path constraint. Directory prefix (src/ or src/foo/), bare filename with extension (main.rs), or glob (*.ts, src/**/*.cc, {src,lib}/**). Applied to the full repo-relative path. Absolute, ~/, and ../ paths outside the workspace are also supported and searched with a separate index.",
      }),
    ),
    exclude: Type.Optional(
      Type.Union([Type.String(), Type.Array(Type.String())], {
        description:
          "Exclude paths (comma/space-separated or array). Same syntax as path: directory prefix ('test/'), filename with extension ('config.json'), or glob ('*.min.js', '**/*.{rs,go}'). A leading '!' is optional and ignored — both 'test/' and '!test/' work. Example: 'test/,*.min.js,!vendor/'.",
      }),
    ),
    caseSensitive: Type.Optional(
      Type.Boolean({
        description:
          "Force case-sensitive matching. Default uses smart-case (case-insensitive when pattern is all lowercase).",
      }),
    ),
    context: Type.Optional(
      Type.Number({
        description: `Context lines before+after each match (0-${GREP_CONTEXT_MAX})`,
      }),
    ),
    limit: Type.Optional(
      Type.Number({
        description: `Max matches (default ${DEFAULT_GREP_LIMIT})`,
      }),
    ),
    cursor: Type.Optional(
      Type.String({ description: "Pagination cursor from previous result" }),
    ),
  });

  pi.registerTool({
    name: toolNames.grep,
    label: toolNames.grep,
    description: `Grep file contents. Smart-case, auto-detects regex vs literal, git-aware. Results are ranked by frecency (most-accessed files first); matches within a file stay in source order. Default limit ${DEFAULT_GREP_LIMIT}.`,
    promptSnippet: "Grep contents",
    promptGuidelines: [
      `${toolNames.grep}: prefer bare identifiers as patterns. Literal queries are most efficient.`,
      `${toolNames.grep}: use path for include ('src/', '*.ts') and exclude for noise ('test/,*.min.js').`,
      `${toolNames.grep}: caseSensitive: true when you need exact case (smart-case otherwise).`,
      `${toolNames.grep}: after 1-2 greps, read the top match instead of more greps.`,
    ],
    parameters: grepSchema,

    async execute(_toolCallId, params, signal) {
      if (signal?.aborted) throw new Error("Operation aborted");

      const pattern = params.pattern;
      const aux = await resolveFinderForPath(params.path, pattern, params.exclude);

      const picker = aux ? aux.finder : await ensureFinder(activeCwd);
      const effectiveLimit = Math.max(1, params.limit ?? DEFAULT_GREP_LIMIT);
      // pageSize caps TOTAL matches across all files; maxMatchesPerFile alone
      // only caps per-file, so limit=5 could still return a full SDK page.
      const pageSize = Math.min(effectiveLimit, GREP_PAGE_SIZE_MAX);
      const context = clampContext(params.context);
      const query = aux
        ? aux.query
        : buildQuery(params.path, pattern, params.exclude, activeCwd);

      // Auto-detect: regex if the pattern has regex metacharacters AND parses
      // as a valid regex, otherwise plain literal. The fuzzy fallback below
      // only kicks in for plain mode — regex queries are intentional.
      const hasRegexSyntax = pattern !== pattern.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

      let mode: GrepMode = hasRegexSyntax ? "regex" : "plain";
      if (mode === "regex") {
        try {
          new RegExp(pattern);
        } catch {
          mode = "plain";
        }
      }

      // Guard: the agent keeps calling grep with '.*' or similar wildcard-only regex
      // to try to read a whole file. That's not what grep is for — return a terse error
      // steering them to a real pattern, preventing dozens of wasted retries.
      const p = pattern.trim();
      const isWildcardOnly =
        hasRegexSyntax &&
        /^(?:[.^$]*(?:[.][*+?]|\*|\+)[.^$]*|[.^$\s]*|\.\*\??|\.\*[+?]?|\.\+\??|\.|\*|\?)$/.test(
          p,
        );

      if (isWildcardOnly) {
        return {
          content: [
            {
              type: "text",
              text: `Pattern '${params.pattern}' matches everything — grep needs a concrete substring or identifier. Example: \`pattern: 'MyClass'\` or \`pattern: 'export function'\`.`,
            },
          ],
          details: { totalMatched: 0, totalFiles: 0 },
        };
      }

      // caseSensitive override flips smartCase off; omitting it keeps smart-case
      // (case-insensitive when pattern is all lowercase).
      const smartCase = params.caseSensitive !== true;

      const grepResult = picker.grep(query, {
        mode,
        smartCase,
        maxMatchesPerFile: pageSize,
        pageSize,
        cursor: (params.cursor ? getCursor(params.cursor) : null) ?? null,
        beforeContext: context,
        afterContext: context,
        classifyDefinitions: true,
        timeBudgetMs: GREP_TIME_BUDGET_MS,
      });

      if (!grepResult.ok) throw new Error(grepResult.error);

      let result = grepResult.value;
      let fuzzyNotice: string | null = null;

      // if we hit the timeout do not run the fuzzy fallback
      // cause it will only consumer more time
      if (
        result.items.length === 0 &&
        !result.nextCursor &&
        !params.cursor &&
        mode !== "regex"
      ) {
        // When the caller pinned a specific file (path has an extension), the
        // fuzzy fallback broadens across the whole picker — the file may just
        // be misnamed. For directory constraints (or no path), we keep the
        // constrained query so the fallback does not leak matches from
        // excluded / out-of-scope directories.
        const lastSeg = params.path?.split(/[\\/]/).pop() ?? "";
        const pathTargetsFile = /\.[a-zA-Z][a-zA-Z0-9]{0,9}$/.test(lastSeg);
        const fuzzyQuery = pathTargetsFile ? pattern : query;
        const fuzzy = picker.grep(fuzzyQuery, {
          mode: "fuzzy",
          smartCase,
          maxMatchesPerFile: pageSize,
          pageSize,
          cursor: null,
          beforeContext: 0,
          afterContext: 0,
          classifyDefinitions: true,
          timeBudgetMs: GREP_TIME_BUDGET_MS,
        });

        if (fuzzy.ok && fuzzy.value.items.length > 0) {
          fuzzyNotice = `0 exact matches. Maybe you meant this?`;
          result = fuzzy.value;
        }
      }

      let output = formatGrepOutput(result);
      const notices: string[] = [];
      if (result.regexFallbackError) {
        notices.push(`Invalid regex: ${result.regexFallbackError}, used literal match`);
      }
      if (result.nextCursor) {
        notices.push(`Continue with cursor="${storeCursor(result.nextCursor)}"`);
      }

      if (notices.length > 0) output += `\n\n[${notices.join(". ")}]`;
      if (fuzzyNotice) output = `[${fuzzyNotice}]\n${output}`;

      return {
        content: [{ type: "text", text: output }],
        details: {
          totalMatched: result.totalMatched,
          totalFiles: result.totalFiles,
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
        "Fuzzy filename search and glob search. Frecency-ranked, git-aware. Multi-word = narrower (AND) not bound to order, use for multi word related concept search. Prefer this over ls/find/bash as the first exploration step whenever the user names a concept, feature, or symbol — it surfaces the relevant files in one call. Only use ls/read on a directory when you specifically need the alphabetical layout of an unknown repo, or when a concept search returned nothing.",
    }),
    path: Type.Optional(
      Type.String({
        description:
          "Path constraint. Directory prefix (src/ or src/foo/), bare filename with extension (main.rs), or glob (*.ts, src/**/*.cc, {src,lib}/**). Applied to the full repo-relative path. Absolute, ~/, and ../ paths outside the workspace are also supported and searched with a separate index.",
      }),
    ),
    exclude: Type.Optional(
      Type.Union([Type.String(), Type.Array(Type.String())], {
        description:
          "Exclude paths (comma/space-separated or array). Same syntax as path: directory prefix ('test/'), filename with extension ('config.json'), or glob ('*.min.js', '**/*.{rs,go}'). A leading '!' is optional and ignored — both 'test/' and '!test/' work. Example: 'test/,*.min.js,!vendor/'.",
      }),
    ),
    limit: Type.Optional(
      Type.Number({
        description: `Max results per page (default ${DEFAULT_FIND_LIMIT})`,
      }),
    ),
    cursor: Type.Optional(
      Type.String({ description: "Pagination cursor from previous result" }),
    ),
  });

  pi.registerTool({
    name: toolNames.find,
    label: toolNames.find,
    description: `Fuzzy path search and glob search. Matches against the whole repo-relative path, not just the filename. Frecency-ranked, git-aware. Multi-word = narrower (AND). Default limit ${DEFAULT_FIND_LIMIT}.`,
    promptSnippet: "Find files by path or glob",
    promptGuidelines: [
      `${toolNames.find}: matches the WHOLE path, not just the filename — \`profile\` hits \`chrome/browser/profiles/x.cc\` too.`,
      `${toolNames.find}: keep queries to 1-2 terms; extra words narrow.`,
      `${toolNames.find}: use for paths, not content. Use ${toolNames.grep} for content.`,
      `${toolNames.find}: for exact path matches use a glob in \`path\` — e.g. path: '**/profile.h' for exact filename, or path: 'src/**/profile.h' scoped to a subtree. Bare patterns are fuzzy.`,
      `${toolNames.find}: to list everything inside a directory, pass path: 'dir/**' with an empty or wildcard pattern instead of using pattern alone.`,
      `${toolNames.find}: use exclude: 'test/,*.min.js' to cut noise in large repos.`,
    ],
    parameters: findSchema,

    async execute(_toolCallId, params, signal) {
      if (signal?.aborted) throw new Error("Operation aborted");

      // if resumed we use the same picker as before
      const resumed = params.cursor ? getFindCursor(params.cursor) : undefined;
      const aux = resumed
        ? resumed.auxRoot
          ? {
              finder: (await auxPool.acquire(resumed.auxRoot, { exact: true })).finder,
              root: resumed.auxRoot,
            }
          : null
        : await resolveFinderForPath(params.path, params.pattern, params.exclude);

      const picker = aux ? aux.finder : await ensureFinder(activeCwd);
      const effectiveLimit = resumed
        ? resumed.pageSize
        : Math.max(1, params.limit ?? DEFAULT_FIND_LIMIT);

      const query = resumed
        ? resumed.query
        : aux && "query" in aux
          ? (aux as { query: string }).query
          : buildQuery(params.path, params.pattern, params.exclude, activeCwd);

      const pattern = resumed ? resumed.pattern : params.pattern;
      const pageIndex = resumed?.nextPageIndex ?? 0;
      const auxRoot = resumed?.auxRoot ?? aux?.root;

      const searchResult = picker.fileSearch(query, {
        pageIndex,
        pageSize: effectiveLimit,
      });
      if (!searchResult.ok) throw new Error(searchResult.error);

      const result = searchResult.value;
      const formatted = formatFindOutput(result, effectiveLimit, pattern);
      let output = formatted.output;

      // Infer hasMore: native fileSearch fills pageSize when more results
      // exist, so if we got a full page AND totalMatched exceeds what we've
      // shown so far there's another page to fetch.
      const shownSoFar = pageIndex * effectiveLimit + result.items.length;
      const hasMore =
        result.items.length >= effectiveLimit && result.totalMatched > shownSoFar;

      const notices: string[] = [];
      if (formatted.weak && formatted.shownCount > 0)
        notices.push(
          `Query "${pattern}" produced only weak scattered fuzzy matches. Output capped at ${formatted.shownCount}/${result.totalMatched}.`,
        );

      if (!formatted.weak && hasMore) {
        const remaining = result.totalMatched - shownSoFar;
        const cursorId = storeFindCursor({
          query,
          pattern,
          pageSize: effectiveLimit,
          nextPageIndex: pageIndex + 1,
          auxRoot,
        });
        notices.push(
          `${remaining} more match${remaining === 1 ? "" : "es"} available. cursor="${cursorId}" to continue`,
        );
      }

      if (notices.length > 0) output += `\n\n[${notices.join(". ")}]`;
      return {
        content: [{ type: "text", text: output }],
        details: {
          totalMatched: result.totalMatched,
          totalFiles: result.totalFiles,
          pageIndex,
          hasMore,
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
      if (args?.cursor) content += theme.fg("muted", ` (page)`);
      text.setText(content);
      return text;
    },

    renderResult(result, options, theme, context) {
      return renderTextResult(result, options, theme, context, 20);
    },
  });

  // --- multi_grep tool ---
  // My latest tests are showing that the multi grep tool is only harmful, trying to get rid of it
  const enableMultiGrep = process.env.PI_FFF_MULTIGREP === "1";

  if (enableMultiGrep) {
    const multiGrepSchema = Type.Object({
      patterns: Type.Array(Type.String(), {
        description:
          "Literal patterns (OR). Include snake_case/camelCase/PascalCase variants.",
      }),
      constraints: Type.Optional(
        Type.String({ description: "File filter, e.g. '*.{ts,tsx} !test/'" }),
      ),
      context: Type.Optional(
        Type.Number({
          description: `Context lines before+after (0-${GREP_CONTEXT_MAX})`,
        }),
      ),
      limit: Type.Optional(
        Type.Number({
          description: `Max matches (default ${DEFAULT_GREP_LIMIT})`,
        }),
      ),
      cursor: Type.Optional(Type.String({ description: "Pagination cursor" })),
    });

    pi.registerTool({
      name: toolNames.multiGrep,
      label: toolNames.multiGrep,
      description:
        "Search file contents for ANY of multiple literal patterns (OR, SIMD Aho-Corasick). Faster than regex alternation.",
      promptSnippet: "Multi-pattern OR content search",
      promptGuidelines: [
        `${toolNames.multiGrep}: use when searching for several identifiers at once.`,
        `${toolNames.multiGrep}: include all naming-convention variants (snake/camel/Pascal).`,
        `${toolNames.multiGrep}: patterns are literal. Use constraints for file filters.`,
      ],
      parameters: multiGrepSchema,

      async execute(_toolCallId, params, signal) {
        if (signal?.aborted) throw new Error("Operation aborted");
        if (!params.patterns?.length)
          throw new Error("patterns array must have at least 1 element");

        const f = await ensureFinder(activeCwd);
        const effectiveLimit = Math.max(1, params.limit ?? DEFAULT_GREP_LIMIT);
        const pageSize = Math.min(effectiveLimit, GREP_PAGE_SIZE_MAX);
        const context = clampContext(params.context);

        const grepResult = f.multiGrep({
          patterns: params.patterns,
          constraints: params.constraints,
          maxMatchesPerFile: pageSize,
          pageSize,
          smartCase: true,
          cursor: (params.cursor ? getCursor(params.cursor) : null) ?? null,
          beforeContext: context,
          afterContext: context,
        });

        if (!grepResult.ok) throw new Error(grepResult.error);

        const result = grepResult.value;
        let output = formatGrepOutput(result);

        const notices: string[] = [];
        if (result.items.length >= effectiveLimit)
          notices.push(`${effectiveLimit}+ matches (refine patterns)`);
        if (result.nextCursor)
          notices.push(
            `More available. cursor="${storeCursor(result.nextCursor)}" to continue`,
          );

        if (notices.length > 0) output += `\n\n[${notices.join(". ")}]`;

        return {
          content: [{ type: "text", text: output }],
          details: {
            totalMatched: result.totalMatched,
            totalFiles: result.totalFiles,
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
  } // end if (enableMultiGrep)

  // --- commands ---

  pi.registerCommand("fff-mode", {
    description: "Show or set FFF mode: /fff-mode [tools-and-ui | tools-only | override]",
    handler: async (args, ctx) => {
      const arg = (args || "").trim();

      // No args - show current mode
      if (!arg) {
        const mode = getMode();
        const flag = pi.getFlag("fff-mode") ?? "unset";
        ctx.ui.notify(`Current mode: '${mode}' (flag: ${flag})`, "info");
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

      pi.appendEntry("fff-mode", { mode: newMode });

      const note =
        (oldMode === "override") !== (newMode === "override")
          ? " (tool name change requires /reload)"
          : "";
      ctx.ui.notify(`Mode changed: '${oldMode}' → '${newMode}'${note}`, "info");
    },
  });

  pi.registerCommand("fff-health", {
    description: "Show FFF file finder health and status",
    handler: async (_args, ctx) => {
      if (!mainFinder || mainFinder.isDestroyed) {
        ctx.ui.notify("FFF not initialized", "warning");
        return;
      }

      const health = mainFinder.healthCheck();
      if (!health.ok) {
        ctx.ui.notify(`Health check failed: ${health.error}`, "error");
        return;
      }

      const lines = [
        `FFF v${health.value.version}`,
        `Mode: ${getMode()}`,
        `Git: ${health.value.git.repositoryFound ? `yes (${health.value.git.workdir ?? "unknown"})` : "no"}`,
        `Picker: ${health.value.filePicker.initialized ? `${health.value.filePicker.indexedFiles ?? 0} files` : "not initialized"}`,
        `Frecency: ${health.value.frecency.initialized ? "active" : "disabled"}`,
        `Query tracker: ${health.value.queryTracker.initialized ? "active" : "disabled"}`,
      ];

      const progress = mainFinder.getScanProgress();
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
      if (!mainFinder || mainFinder.isDestroyed) {
        ctx.ui.notify("FFF not initialized", "warning");
        return;
      }

      const result = mainFinder.scanFiles();
      if (!result.ok) {
        ctx.ui.notify(`Rescan failed: ${result.error}`, "error");
        return;
      }

      ctx.ui.notify("FFF rescan triggered", "info");
    },
  });
}
