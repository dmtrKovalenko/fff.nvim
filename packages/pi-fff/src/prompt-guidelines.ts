/**
 * Prompt guidelines for FFF tools, ported from crates/fff-mcp/src/main.rs MCP_INSTRUCTIONS.
 * Adapted for pi.dev tool names (fff_find_files, fff_grep, fff_multi_grep).
 */

export const FFF_GUIDELINES = [
  "FFF is a fast file finder with frecency-ranked results (frequent/recent files first, git-dirty files boosted).",
  "",
  "## Which Tool Should I Use?",
  "- **fff_grep**: DEFAULT tool. Searches file CONTENTS -- definitions, usage, patterns.",
  "- **fff_find_files**: Explores which files/modules exist. Use when you DON'T have a specific identifier.",
  "- **fff_multi_grep**: OR logic across multiple patterns. Use for case variants.",
  "",
  "## Core Rules",
  "1. Search BARE IDENTIFIERS only -- one identifier per query",
  "2. NEVER use regex unless you truly need alternation. Use fff_multi_grep instead.",
  "3. Stop searching after 2 greps -- READ the code",
  "4. Use fff_multi_grep for multiple identifiers in one call",
  "",
  "## Constraint Syntax (prepend before search text in fff_grep, use 'constraints' param in fff_multi_grep)",
  "  Extension: '*.rs', '*.{ts,tsx}'",
  "  Directory: 'src/', 'quotes/'",
  "  Filename: 'schema.rs'",
  "  Exclude: '!test/', '!*.spec.ts'",
  "  ! Bare words without extensions are NOT constraints.",
].join("\n");
