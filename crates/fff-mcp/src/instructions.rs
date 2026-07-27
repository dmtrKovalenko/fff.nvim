use crate::ExposedTool;

pub fn build_instructions(tools: &[ExposedTool]) -> String {
    let has_find = tools.contains(&ExposedTool::FindFiles);
    let has_grep = tools.contains(&ExposedTool::Grep);
    let has_multi = tools.contains(&ExposedTool::MultiGrep);

    let mut s = String::new();
    s.push_str(
        "FFF is a fast file finder with frecency-ranked results (frequent/recent files first, git-dirty files boosted).\n\n",
    );

    s.push_str("## Which Tool Should I Use?\n\n");
    if has_grep {
        s.push_str("- **grep**: DEFAULT tool. Searches file CONTENTS -- definitions, usage, patterns. Use when you have a specific name or pattern.\n");
    }
    if has_find {
        s.push_str("- **find_files**: Explores which files/modules exist for a topic. Use when you DON'T have a specific identifier or LOOKING FOR A FILE.\n");
    }
    if has_multi {
        s.push_str("- **multi_grep**: OR logic across multiple patterns. Use for case variants (e.g. ['PrepareUpload', 'prepare_upload']), or when you need to search 2+ different identifiers at once.\n");
    }

    if has_grep || has_multi {
        s.push_str("\n## Core Rules\n\n");
        s.push_str("### 1. Search BARE IDENTIFIERS only\n");
        s.push_str("Grep matches single lines. Search for ONE identifier per query:\n");
        s.push_str("  + 'InProgressQuote'           -> finds definition + all usages\n");
        s.push_str("  + 'ActorAuth'                 -> finds enum, struct, all call sites\n");
        s.push_str(
            "  x 'load.*metadata.*InProgressQuote' -> regex spanning multiple tokens, 0 results\n",
        );
        s.push_str("  x 'ctx.data::<ActorAuth>'     -> code syntax, too specific, 0 results\n");
        s.push_str("  x 'struct ActorAuth'          -> adding keywords narrows results, misses enums/traits/type aliases\n");
        s.push_str("  x 'TODO.*#\\d+'               -> complex regex, use simple 'TODO' then filter visually\n\n");

        s.push_str("### 2. NEVER use regex unless you truly need alternation\n");
        s.push_str("Plain text search is faster and more reliable. Regex patterns like `.*`, `\\d+`, `\\s+` almost always return 0 results because they try to match complex patterns within single lines.\n");
        if has_multi {
            s.push_str("If you need OR logic, use multi_grep with literal patterns instead of regex alternation.\n");
        }
        s.push('\n');

        s.push_str("### 3. Stop searching after 2 greps -- READ the code\n");
        s.push_str("After 2 grep calls, you have enough file paths. Read the top result to understand the code.\n");
        s.push_str("Do NOT keep grepping with variations. More greps != better understanding.\n\n");

        if has_multi {
            s.push_str("### 4. Use multi_grep for multiple identifiers\n");
            s.push_str("When you need to find different names (e.g. snake_case + PascalCase, or definition + usage patterns), use ONE multi_grep call instead of sequential greps:\n");
            s.push_str("  + multi_grep(['ActorAuth', 'PopulatedActorAuth', 'actor_auth'])\n");
            s.push_str("  x grep 'ActorAuth' -> grep 'PopulatedActorAuth' -> grep 'actor_auth'  (3 calls wasted)\n\n");
        }
    }

    s.push_str("## Workflow\n\n");
    if has_grep {
        s.push_str("**Have a specific name?** -> grep the bare identifier.\n");
    }
    if has_multi {
        s.push_str(
            "**Need multiple name variants?** -> multi_grep with all variants in one call.\n",
        );
    }
    if has_find {
        s.push_str("**Exploring a topic / finding files?** -> find_files.\n");
    }
    if has_grep || has_multi {
        s.push_str("**Got results?** -> Read the top file. Don't grep again.\n");
    }

    if has_grep || has_multi {
        s.push_str("\n## Constraint Syntax\n\n");
        if has_grep {
            s.push_str("For grep: constraints go INLINE, prepended before the search text.\n");
        }
        if has_multi {
            s.push_str("For multi_grep: constraints go in the separate 'constraints' parameter.\n");
        }
        s.push('\n');

        s.push_str("Constraints MUST match one of these formats:\n");
        s.push_str("  Extension: '*.rs', '*.{ts,tsx}'\n");
        s.push_str("  Directory: 'src/', 'quotes/'\n");
        s.push_str("  Filename: 'schema.rs', 'src/main.rs'\n");
        s.push_str("  Exclude: '!test/', '!*.spec.ts'\n\n");

        s.push_str("! Bare words without extensions are NOT constraints. 'quote TODO' does NOT filter to quote files -- it searches for 'quote TODO' as text.\n");
        s.push_str("  + 'schema.rs TODO'   -> searches for 'TODO' in files schema.rs\n");
        s.push_str("  + 'quotes/ TODO'     -> searches for 'TODO' in the quotes/ directory\n");
        s.push_str(
            "  x 'quote TODO'       -> searches for literal text 'quote TODO', finds nothing\n\n",
        );

        s.push_str("Prefer broad constraints:\n");
        s.push_str("  + '*.rs query'           -> file type\n");
        s.push_str("  + 'quotes/ query'        -> top-level dir\n");
        s.push_str("  x 'quotes/storage/db/ query' -> too specific, misses results\n\n");

        s.push_str("## Output Format\n\n");
        s.push_str("grep results auto-expand definitions with body context (struct fields, function signatures).\n");
        s.push_str("This often provides enough information WITHOUT a follow-up Read call.\n");
        s.push_str(
            "Lines marked with | are definition body context. [def] marks definition files.\n",
        );
        s.push_str("-> Read suggestions point to the most relevant file -- follow them when you need more context.\n\n");

        s.push_str("## Default Exclusions\n\n");
        s.push_str("If results are cluttered with irrelevant files, exclude them:\n");
        s.push_str("  !tests/ - exclude tests directory\n");
        s.push_str("  !*.spec.ts - exclude test files\n");
        s.push_str("  !generated/ - exclude generated code");
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_all_tools_mentions_all_three() {
        let s = build_instructions(&ExposedTool::all());
        assert!(s.contains("**find_files**"));
        assert!(s.contains("**grep**"));
        assert!(s.contains("**multi_grep**"));
        assert!(s.contains("### 4. Use multi_grep"));
    }

    #[test]
    fn only_find_files_drops_grep_and_multi_grep_sections() {
        let s = build_instructions(&[ExposedTool::FindFiles]);
        assert!(s.contains("**find_files**"));
        assert!(!s.contains("**grep**"));
        assert!(!s.contains("**multi_grep**"));
        assert!(!s.contains("Constraint Syntax"));
        assert!(!s.contains("Core Rules"));
    }

    #[test]
    fn grep_without_multi_grep_drops_multi_grep_rule() {
        let s = build_instructions(&[ExposedTool::Grep]);
        assert!(s.contains("**grep**"));
        assert!(!s.contains("**multi_grep**"));
        assert!(!s.contains("### 4. Use multi_grep"));
        assert!(!s.contains("use multi_grep with literal patterns"));
        assert!(s.contains("Constraint Syntax"));
    }

    #[test]
    fn multi_grep_only_keeps_multi_grep_rules() {
        let s = build_instructions(&[ExposedTool::MultiGrep]);
        assert!(!s.contains("**grep**:"));
        assert!(s.contains("**multi_grep**"));
        assert!(s.contains("### 4. Use multi_grep"));
    }
}
