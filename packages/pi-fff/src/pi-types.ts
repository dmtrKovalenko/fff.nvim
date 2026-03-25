/**
 * Minimal type declarations for the pi.dev extension API.
 * Full types come from @mariozechner/pi-coding-agent peer dependency at build time.
 * These are used for development without installing pi.dev locally.
 */

export interface ToolContext {
  cwd: string;
  hasUI: boolean;
  ui: UIContext;
}

export interface UIContext {
  notify(message: string, level: "info" | "warning" | "error"): void;
}

export interface AutocompleteItem {
  value: string;
  label: string;
}

export interface ToolRegistration {
  name: string;
  label: string;
  description: string;
  promptSnippet?: string;
  promptGuidelines?: string[];
  parameters: Record<string, unknown>;
  execute(
    toolCallId: string,
    params: any,
    signal: AbortSignal,
    onUpdate: (text: string) => void,
    ctx: ToolContext,
  ): Promise<ToolResult>;
}

export interface ToolResult {
  content: Array<{ type: "text"; text: string }>;
  details?: Record<string, unknown>;
}

export interface CommandRegistration {
  description: string;
  getArgumentCompletions?(prefix: string): AutocompleteItem[] | null;
  handler(args: string, ctx: ToolContext): Promise<void>;
}

export interface ExtensionAPI {
  registerTool(tool: ToolRegistration): void;
  registerCommand(name: string, command: CommandRegistration): void;
  on(
    event: string,
    handler: (event: any, ctx: ToolContext) => void | Promise<void>,
  ): void;
}
