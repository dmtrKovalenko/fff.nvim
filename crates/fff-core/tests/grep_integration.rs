use std::fs;
use std::path::Path;
use tempfile::TempDir;

use fff_search::ContentCacheBudget;
use fff_search::grep::{GrepMode, GrepSearchOptions, grep_search, parse_grep_query};
use fff_search::types::FileItem;

/// Create a file inside a temp dir and return its `FileItem`.
fn create_file(base: &Path, relative: &str, contents: &str) -> FileItem {
    let full_path = base.join(relative);
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&full_path, contents).unwrap();
    FileItem::new(full_path, base, None)
}

/// Shorthand to build default options for plain text mode.
fn plain_opts() -> GrepSearchOptions {
    GrepSearchOptions {
        max_file_size: 10 * 1024 * 1024,
        max_matches_per_file: 200,
        smart_case: true,
        file_offset: 0,
        page_limit: 200,
        mode: GrepMode::PlainText,
        time_budget_ms: 0,
        before_context: 0,
        after_context: 0,
        classify_definitions: false,
        trim_whitespace: false,
    }
}

/// Shorthand to build default options for regex mode.
fn regex_opts() -> GrepSearchOptions {
    GrepSearchOptions {
        max_file_size: 10 * 1024 * 1024,
        max_matches_per_file: 200,
        smart_case: true,
        file_offset: 0,
        page_limit: 200,
        mode: GrepMode::Regex,
        time_budget_ms: 0,
        before_context: 0,
        after_context: 0,
        classify_definitions: false,
        trim_whitespace: false,
    }
}

/// Shorthand to build default options for fuzzy mode.
fn fuzzy_opts() -> GrepSearchOptions {
    GrepSearchOptions {
        max_file_size: 10 * 1024 * 1024,
        max_matches_per_file: 200,
        smart_case: true,
        file_offset: 0,
        page_limit: 200,
        mode: GrepMode::Fuzzy,
        time_budget_ms: 0,
        before_context: 0,
        after_context: 0,
        classify_definitions: false,
        trim_whitespace: false,
    }
}

#[test]
fn plain_text_finds_exact_literal() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "hello.txt",
        "Hello, World!\nGoodbye, World!\n",
    )];

    let parsed = parse_grep_query("Hello");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].line_number, 1);
    assert!(result.matches[0].line_content.contains("Hello"));
}

#[test]
fn plain_text_smart_case_insensitive() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "Hello World\nhello world\nHELLO WORLD\n",
    )];

    // All lowercase query → smart case → case-insensitive
    let parsed = parse_grep_query("hello");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        3,
        "smart case should match all 3 lines"
    );
}

#[test]
fn plain_text_smart_case_sensitive_with_uppercase() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "Hello World\nhello world\nHELLO WORLD\n",
    )];

    // Query has uppercase → smart case → case-sensitive
    let parsed = parse_grep_query("Hello");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        1,
        "uppercase in query should trigger case-sensitive"
    );
    assert_eq!(result.matches[0].line_number, 1);
}

#[test]
fn plain_text_regex_metacharacters_are_literal() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "code.rs",
        "fn main() {\n    println!(\"test\");\n}\nfn foo() {}\n",
    )];

    // In plain text mode, these regex metacharacters should be literal
    let parsed = parse_grep_query("fn main()");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].line_number, 1);

    // Parentheses should NOT be treated as regex groups
    let parsed2 = parse_grep_query("(\"test\")");
    let result2 = grep_search(
        &files,
        &parsed2,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    assert_eq!(result2.matches.len(), 1);
    assert_eq!(result2.matches[0].line_number, 2);
}

#[test]
fn plain_text_dot_is_literal() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "config.toml",
        "version = \"1.0\"\nname = \"foo\"\nversion_major = 1\n",
    )];

    // In plain text mode, dot should be literal, not "any char"
    let parsed = parse_grep_query("1.0");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        1,
        "dot should be literal, not 'any char'"
    );
    assert!(result.matches[0].line_content.contains("1.0"));
}

#[test]
fn plain_text_asterisk_is_literal() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "doc.md",
        "Use **bold** text\nUse *italic* text\nUse normal text\n",
    )];

    let parsed = parse_grep_query("**bold**");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].line_number, 1);
}

#[test]
fn plain_text_backslash_is_literal() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "paths.txt",
        "C:\\Users\\foo\\bar\n/home/user/bin\n",
    )];

    let parsed = parse_grep_query("C:\\Users");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    assert_eq!(result.matches.len(), 1);
}

#[test]
fn plain_text_across_multiple_files() {
    let tmp = TempDir::new().unwrap();
    let files = vec![
        create_file(tmp.path(), "a.txt", "use std::io;\nuse std::fs;\n"),
        create_file(tmp.path(), "b.txt", "use std::path;\nuse serde;\n"),
        create_file(tmp.path(), "c.txt", "no match here\n"),
    ];

    let parsed = parse_grep_query("use std");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 3);
    // Should match in files a.txt and b.txt
    assert_eq!(result.files.len(), 2);
}

#[test]
fn plain_text_highlight_offsets_are_correct() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(tmp.path(), "a.txt", "foo bar foo baz foo\n")];

    let parsed = parse_grep_query("foo");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 1);
    let m = &result.matches[0];
    // "foo" appears at byte offsets 0, 8, 16
    assert_eq!(m.match_byte_offsets.len(), 3);
    assert_eq!(m.match_byte_offsets[0], (0, 3));
    assert_eq!(m.match_byte_offsets[1], (8, 11));
    assert_eq!(m.match_byte_offsets[2], (16, 19));
    assert_eq!(m.col, 0);
}

#[test]
fn plain_text_empty_query_returns_no_content_matches() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(tmp.path(), "a.txt", "some content\n")];

    let parsed = parse_grep_query("");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    // Empty query in grep returns git-modified welcome state (no content matches)
    // Since our test files have no git status, we expect 0 matches
    assert_eq!(result.matches.len(), 0);
}

#[test]
fn plain_text_binary_files_are_skipped() {
    let tmp = TempDir::new().unwrap();
    let binary_path = tmp.path().join("binary.dat");
    let mut content = b"match this text\n".to_vec();
    content.extend_from_slice(&[0u8; 100]); // NUL bytes make it binary
    content.extend_from_slice(b"match this text\n");
    fs::write(&binary_path, &content).unwrap();
    // In production, binary detection by content happens during bigram build
    // and sets is_binary = true. Simulate that here with new_raw.
    let meta = fs::metadata(&binary_path).unwrap();
    let binary_file = {
        let p = binary_path.to_string_lossy().into_owned();
        let rs = (p.len() - "binary.dat".len()) as u16;
        FileItem::new_raw(p, rs, rs, meta.len(), 0, None, true)
    };

    let text_file = create_file(tmp.path(), "text.txt", "match this text\n");

    let files = vec![binary_file, text_file];

    let parsed = parse_grep_query("match this text");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    // Only the text file should be searched, not the binary one
    assert_eq!(result.files.len(), 1);
    assert!(result.files[0].relative_path().contains("text.txt"));
}

#[test]
fn plain_text_max_matches_per_file() {
    let tmp = TempDir::new().unwrap();
    let mut content = String::new();
    for i in 0..50 {
        content.push_str(&format!("line {} match_target\n", i));
    }
    let files = vec![create_file(tmp.path(), "many.txt", &content)];

    let mut opts = plain_opts();
    opts.max_matches_per_file = 5;

    let parsed = parse_grep_query("match_target");
    let result = grep_search(
        &files,
        &parsed,
        &opts,
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        5,
        "should cap at max_matches_per_file"
    );
}

#[test]
fn plain_text_page_limit() {
    let tmp = TempDir::new().unwrap();
    let mut content = String::new();
    for i in 0..100 {
        content.push_str(&format!("line {} target\n", i));
    }
    let files = vec![create_file(tmp.path(), "big.txt", &content)];

    let mut opts = plain_opts();
    opts.page_limit = 10;

    let parsed = parse_grep_query("target");
    let result = grep_search(
        &files,
        &parsed,
        &opts,
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    // page_limit is a soft minimum: we always finish the current file, so we
    // get at least page_limit matches (no data loss) and at most
    // max_matches_per_file (200) from a single file.
    assert!(
        result.matches.len() >= opts.page_limit,
        "should return at least page_limit matches: got {}",
        result.matches.len()
    );
    assert!(
        result.matches.len() <= opts.max_matches_per_file,
        "should never exceed max_matches_per_file: got {}",
        result.matches.len()
    );
    // Single file with 100 lines all matching — all should be returned.
    assert_eq!(result.matches.len(), 100, "all 100 lines must be returned");
}

#[test]
fn plain_text_file_offset_pagination() {
    let tmp = TempDir::new().unwrap();
    // Create many files (1 match per file) so file-based pagination exercises
    // offset tracking across files with and without matches.
    let mut files = Vec::new();
    for i in 0..20 {
        files.push(create_file(
            tmp.path(),
            &format!("file_{:02}.txt", i),
            &format!("unique_token_{}\n", i),
        ));
    }

    let mut opts = plain_opts();
    opts.page_limit = 5;

    // Collect ALL matches across all pages and verify no duplicates and full coverage.
    let mut all_line_texts: Vec<String> = Vec::new();
    let mut pages = 0;
    let max_pages = 20; // safety limit

    loop {
        let parsed = parse_grep_query("unique_token");
        let result = grep_search(
            &files,
            &parsed,
            &opts,
            &ContentCacheBudget::unlimited(),
            None,
            None,
            None,
        );

        for m in &result.matches {
            let text = m.line_content.trim().to_string();
            assert!(
                !all_line_texts.contains(&text),
                "duplicate match across pages: '{}'",
                text
            );
            all_line_texts.push(text);
        }

        pages += 1;
        assert!(pages <= max_pages, "pagination did not terminate");

        if result.next_file_offset == 0 {
            break;
        }

        // Offset must strictly advance
        assert!(
            result.next_file_offset > opts.file_offset,
            "next_file_offset ({}) did not advance past current ({})",
            result.next_file_offset,
            opts.file_offset
        );
        opts.file_offset = result.next_file_offset;
    }

    assert_eq!(
        all_line_texts.len(),
        20,
        "pagination should find all 20 matches across all pages, got {}",
        all_line_texts.len()
    );
    assert!(
        pages > 1,
        "should require multiple pages with page_limit=5 and 20 files"
    );
}

#[test]
fn plain_text_line_numbers_are_correct() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "line one\nline two\nline three\nline four\n",
    )];

    let parsed = parse_grep_query("line");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 4);
    assert_eq!(result.matches[0].line_number, 1);
    assert_eq!(result.matches[1].line_number, 2);
    assert_eq!(result.matches[2].line_number, 3);
    assert_eq!(result.matches[3].line_number, 4);
}

#[test]
fn plain_text_max_file_size_filter() {
    let tmp = TempDir::new().unwrap();
    // Create a file larger than 100 bytes
    let big_content = "a".repeat(200) + "\nmatch_me\n";
    let files = vec![create_file(tmp.path(), "big.txt", &big_content)];

    let mut opts = plain_opts();
    opts.max_file_size = 100; // Only allow files up to 100 bytes

    let parsed = parse_grep_query("match_me");
    let result = grep_search(
        &files,
        &parsed,
        &opts,
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 0, "large file should be filtered out");
    assert_eq!(result.filtered_file_count, 0);
}

// ── Regex mode tests ───────────────────────────────────────────────────

#[test]
fn regex_basic_pattern() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "foo123\nbar456\nbaz789\nfoo_bar\n",
    )];

    let parsed = parse_grep_query("foo\\d+");
    let result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].line_number, 1);
    assert!(result.matches[0].line_content.contains("foo123"));
}

#[test]
fn regex_capture_group_matching() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(tmp.path(), "a.txt", "foobar\nfoobaz\nfoo123\n")];

    // Use a capturing group (not lookahead, which regex crate doesn't support)
    let parsed = parse_grep_query("foo(bar|baz)");
    let result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 2);
    let contents: Vec<&str> = result
        .matches
        .iter()
        .map(|m| m.line_content.as_str())
        .collect();
    assert!(contents.contains(&"foobar"));
    assert!(contents.contains(&"foobaz"));
}

#[test]
fn regex_dot_matches_any_char() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "v1.0\nv1x0\nv1-0\nv100\nv2.0\n",
    )];

    // In regex mode, . matches any character, so v1.0 matches v1.0, v1x0, v1-0, and v100
    let parsed = parse_grep_query("v1.0");
    let result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        4,
        "regex dot should match v1.0, v1x0, v1-0, and v100 (dot matches any char)"
    );
}

#[test]
fn regex_alternation() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "apple\nbanana\ncherry\napricot\n",
    )];

    let parsed = parse_grep_query("apple|cherry");
    let result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 2);
    let lines: Vec<u64> = result.matches.iter().map(|m| m.line_number).collect();
    assert!(lines.contains(&1));
    assert!(lines.contains(&3));
}

#[test]
fn regex_character_class() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "cat\ncut\ncot\ncit\ncxt\n",
    )];

    let parsed = parse_grep_query("c[aou]t");
    let result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 3);
    let contents: Vec<&str> = result
        .matches
        .iter()
        .map(|m| m.line_content.as_str())
        .collect();
    assert!(contents.contains(&"cat"));
    assert!(contents.contains(&"cut"));
    assert!(contents.contains(&"cot"));
}

#[test]
fn regex_quantifiers() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "fo\nfoo\nfooo\nfoooo\nbar\n",
    )];

    let parsed = parse_grep_query("fo{2,3}");
    let result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 3, "should match foo, fooo, foooo");
}

#[test]
fn regex_anchors() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "start of line\nmiddle start end\nend of line\n",
    )];

    let parsed = parse_grep_query("^start");
    let result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].line_number, 1);
}

#[test]
fn regex_anchors_multiword() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "test.c",
        "int ff_function(void);\nstatic int ff_other(void);\nint main(void);\nint ff_another(void);\n",
    )];

    // ^int ff_ should match lines starting with "int ff_"
    let parsed = parse_grep_query("^int ff_");
    let result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        2,
        "should match 2 lines starting with 'int ff_'"
    );
    assert!(result.matches[0].line_content.contains("ff_function"));
    assert!(result.matches[1].line_content.contains("ff_another"));
}

#[test]
fn regex_highlight_offsets_variable_length() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(tmp.path(), "a.txt", "aab aaab aaaab\n")];

    let parsed = parse_grep_query("a+b");
    let result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 1);
    let m = &result.matches[0];
    // Regex "a+b" matches "aab" (3 bytes), "aaab" (4 bytes), "aaaab" (5 bytes)
    assert_eq!(m.match_byte_offsets.len(), 3);
    // Verify the match spans have different lengths (variable-length regex)
    assert_eq!(m.match_byte_offsets[0], (0, 3)); // "aab"
    assert_eq!(m.match_byte_offsets[1], (4, 8)); // "aaab"
    assert_eq!(m.match_byte_offsets[2], (9, 14)); // "aaaab"
}

#[test]
fn regex_invalid_pattern_falls_back_to_literal() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "call name(arg)\nother line\n",
    )];

    // Invalid regex: unmatched group — should fall back to literal search
    let parsed = parse_grep_query("name(");
    let result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    // Fallback to literal: finds "name(" in "call name(arg)"
    assert_eq!(
        result.matches.len(),
        1,
        "invalid regex should fall back to literal and find the match"
    );
    assert!(
        result.regex_fallback_error.is_some(),
        "should report the regex compilation error"
    );
    assert!(result.matches[0].line_content.contains("name("));

    // A pattern that doesn't exist anywhere — still falls back but finds nothing
    let parsed2 = parse_grep_query("zzz(");
    let result2 = grep_search(
        &files,
        &parsed2,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    assert_eq!(result2.matches.len(), 0);
    assert!(result2.regex_fallback_error.is_some());
}

#[test]
fn regex_smart_case() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "Foo bar\nfoo bar\nFOO BAR\n",
    )];

    // Lowercase query → case-insensitive
    let parsed_lower = parse_grep_query("foo");
    let result_lower = grep_search(
        &files,
        &parsed_lower,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    assert_eq!(result_lower.matches.len(), 3);

    // Query with uppercase → case-sensitive
    let parsed_upper = parse_grep_query("Foo");
    let result_upper = grep_search(
        &files,
        &parsed_upper,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    assert_eq!(result_upper.matches.len(), 1);
}

#[test]
fn regex_across_multiple_files() {
    let tmp = TempDir::new().unwrap();
    let files = vec![
        create_file(
            tmp.path(),
            "lib.rs",
            "fn main() {}\nfn helper() {}\nstruct Foo;\n",
        ),
        create_file(
            tmp.path(),
            "test.rs",
            "fn test_one() {}\nfn test_two() {}\nmod tests;\n",
        ),
        create_file(tmp.path(), "readme.md", "# Title\nSome text\n"),
    ];

    let parsed = parse_grep_query("fn \\w+\\(\\)");
    let result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    // Should match: fn main(), fn helper(), fn test_one(), fn test_two()
    assert_eq!(result.matches.len(), 4);
    assert_eq!(result.files.len(), 2, "matches in 2 .rs files, not readme");
}

// ── Mode comparison tests ──────────────────────────────────────────────

#[test]
fn plain_text_and_regex_agree_on_simple_literal() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "hello world\ngoodbye world\nhello again\n",
    )];

    let parsed = parse_grep_query("hello");
    let plain_result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    let regex_result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(plain_result.matches.len(), regex_result.matches.len());
    for (p, r) in plain_result.matches.iter().zip(regex_result.matches.iter()) {
        assert_eq!(p.line_number, r.line_number);
        assert_eq!(p.line_content, r.line_content);
    }
}

#[test]
fn plain_text_escapes_what_regex_does_not() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "price is $100\nprice is 100\nprice is $200\n",
    )];

    // "$100" — in plain text, $ is literal; in regex, $ is anchor
    let parsed_plain = parse_grep_query("$100");
    let plain_result = grep_search(
        &files,
        &parsed_plain,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    let parsed_regex = parse_grep_query("\\$100");
    let regex_result = grep_search(
        &files,
        &parsed_regex,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    // Plain text should find "$100" literally
    assert_eq!(plain_result.matches.len(), 1);
    assert!(plain_result.matches[0].line_content.contains("$100"));

    // Regex with escaped $ should also find "$100"
    assert_eq!(regex_result.matches.len(), 1);
}

// ── Constraint integration tests ───────────────────────────────────────

#[test]
fn grep_with_extension_constraint() {
    let tmp = TempDir::new().unwrap();
    let files = vec![
        create_file(tmp.path(), "a.rs", "use std::io;\nfn main() {}\n"),
        create_file(tmp.path(), "b.txt", "use std::io;\nsome text\n"),
        create_file(tmp.path(), "c.rs", "use std::fs;\n"),
    ];

    let parsed = parse_grep_query("use std *.rs");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    // Should only search .rs files
    for file in &result.files {
        assert!(
            file.relative_path().ends_with(".rs"),
            "should only match .rs files, got: {}",
            file.relative_path()
        );
    }
    assert!(
        result.matches.len() >= 2,
        "should find matches in .rs files"
    );
}

// ── Bracket / glob character tests ─────────────────────────────────────

#[test]
fn plain_text_bracket_is_literal() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "code.rs",
        "let x = arr[0];\nlet y = arr[1];\nlet z = something;\n",
    )];

    let parsed = parse_grep_query("arr[0]");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        1,
        "brackets should be literal in plain text mode"
    );
    assert_eq!(result.matches[0].line_number, 1);
}

// ── Backslash escape tests ─────────────────────────────────────────────

#[test]
fn grep_backslash_escapes_extension_filter() {
    let tmp = TempDir::new().unwrap();
    let files = vec![
        create_file(tmp.path(), "a.rs", "contains *.rs pattern\n"),
        create_file(tmp.path(), "b.txt", "also has *.rs here\n"),
    ];

    // Without escape: "*.rs" is an extension filter, so only .rs files are searched
    let parsed = parse_grep_query("pattern *.rs");
    let result_filter = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    assert_eq!(
        result_filter.files.len(),
        1,
        "*.rs should filter to .rs files"
    );

    // With escape: "\*.rs" is literal text, both files are searched
    let parsed_escaped = parse_grep_query("\\*.rs");
    let result_literal = grep_search(
        &files,
        &parsed_escaped,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    assert_eq!(
        result_literal.matches.len(),
        2,
        "\\*.rs should search for literal *.rs in all files"
    );
}

#[test]
fn grep_backslash_escapes_path_segment() {
    let tmp = TempDir::new().unwrap();
    let files = vec![
        create_file(tmp.path(), "src/main.rs", "search for /src/ path\n"),
        create_file(tmp.path(), "lib/utils.rs", "also /src/ mentioned\n"),
    ];

    // With escape: "\\/src/" is literal text, not a path constraint
    let parsed = parse_grep_query("\\/src/");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    assert_eq!(
        result.matches.len(),
        2,
        "\\/src/ should search for literal /src/ in all files"
    );
}

#[test]
fn grep_backslash_escapes_negation() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "the !test macro\nother stuff\n",
    )];

    // With escape: "\\!test" is literal text "!test"
    let parsed = parse_grep_query("\\!test");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    assert_eq!(result.matches.len(), 1);
    assert!(result.matches[0].line_content.contains("!test"));
}

#[test]
fn grep_with_path_constraint() {
    let tmp = TempDir::new().unwrap();
    let files = vec![
        create_file(tmp.path(), "src/lib.rs", "target_text\n"),
        create_file(tmp.path(), "tests/test.rs", "target_text\n"),
        create_file(tmp.path(), "src/main.rs", "other content\n"),
    ];

    let parsed = parse_grep_query("target_text /src/");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 1);
    assert!(result.files[0].relative_path().starts_with("src/"));
}

// ── Negated constraint tests ───────────────────────────────────────────

#[test]
fn grep_with_negated_extension_constraint() {
    let tmp = TempDir::new().unwrap();
    let files = vec![
        create_file(tmp.path(), "src/lib.rs", "target_text\n"),
        create_file(tmp.path(), "src/app.ts", "target_text\n"),
        create_file(tmp.path(), "src/main.rs", "target_text\n"),
    ];

    let query = "target_text !*.rs";
    let parsed = parse_grep_query(query);
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        1,
        "should only find matches in non-.rs files, got {} matches",
        result.matches.len()
    );
    assert!(
        result.files[0].relative_path().ends_with(".ts"),
        "should only match .ts file, got: {}",
        result.files[0].relative_path()
    );
}

#[test]
fn grep_with_negated_path_constraint() {
    let tmp = TempDir::new().unwrap();
    let files = vec![
        create_file(tmp.path(), "src/lib.rs", "target_text\n"),
        create_file(tmp.path(), "tests/test.rs", "target_text\n"),
        create_file(tmp.path(), "src/main.rs", "other content\n"),
    ];

    let query = "target_text !/src/";
    let parsed = parse_grep_query(query);
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        1,
        "should only find matches outside src/, got {} matches",
        result.matches.len()
    );
    assert!(
        result.files[0].relative_path().starts_with("tests/"),
        "should only match tests/ file, got: {}",
        result.files[0].relative_path()
    );
}

#[test]
fn grep_with_negated_text_constraint() {
    let tmp = TempDir::new().unwrap();
    let files = vec![
        create_file(tmp.path(), "src/lib.rs", "target_text\n"),
        create_file(tmp.path(), "tests/helper.rs", "target_text\n"),
        create_file(tmp.path(), "docs/readme.md", "target_text\n"),
    ];

    let query = "target_text !test";
    let parsed = parse_grep_query(query);
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    // "tests/helper.rs" contains "test" in path, should be excluded
    assert_eq!(
        result.matches.len(),
        2,
        "should find matches in files without 'test' in path, got {} matches",
        result.matches.len()
    );
    for file in &result.files {
        assert!(
            !file.relative_path().contains("test"),
            "should not match files with 'test' in path, got: {}",
            file.relative_path()
        );
    }
}

// ── Edge case tests ────────────────────────────────────────────────────

#[test]
fn grep_empty_file_is_skipped() {
    let tmp = TempDir::new().unwrap();
    let empty_path = tmp.path().join("empty.txt");
    fs::write(&empty_path, "").unwrap();
    let empty_file = FileItem::new(empty_path, tmp.path(), None);

    let text_file = create_file(tmp.path(), "text.txt", "findme\n");

    let files = vec![empty_file, text_file];
    let parsed = parse_grep_query("findme");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 1);
}

#[test]
fn grep_single_line_no_trailing_newline() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(tmp.path(), "a.txt", "no newline at end")];

    let parsed = parse_grep_query("no newline");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].line_number, 1);
}

#[test]
fn grep_unicode_content() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "utf8.txt",
        "日本語テスト\nrégulière\nñoño\n",
    )];

    let parsed = parse_grep_query("régulière");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].line_number, 2);

    let parsed2 = parse_grep_query("ñoño");
    let result2 = grep_search(
        &files,
        &parsed2,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );
    assert_eq!(result2.matches.len(), 1);
    assert_eq!(result2.matches[0].line_number, 3);
}

#[test]
fn grep_long_line_is_truncated() {
    let tmp = TempDir::new().unwrap();
    let long_line = "x".repeat(1000) + "NEEDLE" + &"y".repeat(1000);
    let files = vec![create_file(tmp.path(), "long.txt", &long_line)];

    let parsed = parse_grep_query("NEEDLE");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 1);
    // The line_content should be truncated to MAX_LINE_DISPLAY_LEN (512)
    assert!(
        result.matches[0].line_content.len() <= 512,
        "line should be truncated: len={}",
        result.matches[0].line_content.len()
    );
}

#[test]
fn regex_word_boundary() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "foo\nfoobar\nbarfoo\nfoo_baz\n",
    )];

    let parsed = parse_grep_query("\\bfoo\\b");
    let result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        1,
        "only exact word 'foo' should match"
    );
    assert_eq!(result.matches[0].line_number, 1);
}

#[test]
fn plain_text_question_mark_is_literal() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "what is this?\nhow does it work?\nno question here\nwhat?\n",
    )];

    let parsed = parse_grep_query("?");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        3,
        "question mark should be literal in plain text mode"
    );
}

#[test]
fn plain_text_query_with_question_mark_in_word() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "code.rs",
        "let x = foo?;\nlet y = bar.baz();\nfoo?.unwrap()\n",
    )];

    let parsed = parse_grep_query("foo?");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        2,
        "should find 'foo?' literally in both lines"
    );
}

#[test]
fn regex_question_mark_is_quantifier() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(tmp.path(), "a.txt", "color\ncolour\ncolouur\n")];

    // In regex mode, ? means "zero or one of preceding"
    let parsed = parse_grep_query("colou?r");
    let result = grep_search(
        &files,
        &parsed,
        &regex_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        2,
        "regex ? should match 'color' and 'colour' but not 'colouur'"
    );
}

// ── Fuzzy mode tests ───────────────────────────────────────────────────

#[test]
fn fuzzy_finds_exact_substring() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "a.txt",
        "hello world\ngoodbye world\nhello again\n",
    )];

    let parsed = parse_grep_query("hello");
    let result = grep_search(
        &files,
        &parsed,
        &fuzzy_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        2,
        "fuzzy should find 'hello' in both lines"
    );
    assert!(result.matches[0].line_content.contains("hello"));
    assert!(result.matches[1].line_content.contains("hello"));
}

#[test]
fn fuzzy_finds_scattered_characters() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "code.rs",
        "fn mutex_lock() {}\nfn main() {}\nfn mutex_unlock() {}\n",
    )];

    // "mutex" should fuzzy match "mutex_lock" (contiguous prefix)
    let parsed = parse_grep_query("mutex");
    let result = grep_search(
        &files,
        &parsed,
        &fuzzy_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert!(
        !result.matches.is_empty(),
        "fuzzy should find 'mutex' in 'mutex_lock'"
    );
    assert!(result.matches[0].line_content.contains("mutex_lock"));
}

#[test]
fn fuzzy_highlight_offsets_correct() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(tmp.path(), "a.txt", "hello world\n")];

    let parsed = parse_grep_query("hell");
    let result = grep_search(
        &files,
        &parsed,
        &fuzzy_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 1);
    let m = &result.matches[0];

    // "hell" should match 'h'(0), 'e'(1), 'l'(2), 'l'(3) in "hello"
    // These should be converted to byte offsets
    assert!(
        !m.match_byte_offsets.is_empty(),
        "should have highlight offsets"
    );
}

#[test]
fn fuzzy_unicode_char_indices() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "utf8.txt",
        "日本語テスト\nrégulière\nñoño\n",
    )];

    // Use "guli" which is a contiguous ASCII substring within "régulière"
    // (the chars g-u-l-i appear contiguously between the two accented chars)
    let parsed = parse_grep_query("guli");
    let result = grep_search(
        &files,
        &parsed,
        &fuzzy_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    // Should fuzzy match "régulière" (with multi-byte é and è)
    // This tests that character-to-byte offset conversion works with UTF-8
    assert!(!result.matches.is_empty());
    assert!(result.matches[0].line_content.contains("régulière"));
}

#[test]
fn fuzzy_empty_query_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(tmp.path(), "a.txt", "some content\n")];

    let parsed = parse_grep_query("");
    let result = grep_search(
        &files,
        &parsed,
        &fuzzy_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    // Empty query returns git-modified files, not fuzzy matches
    assert_eq!(result.matches.len(), 0);
}

#[test]
fn fuzzy_with_extension_constraint() {
    let tmp = TempDir::new().unwrap();
    let files = vec![
        create_file(tmp.path(), "a.rs", "use std::io;\nfn main() {}\n"),
        create_file(tmp.path(), "b.txt", "use std::io;\nsome text\n"),
        create_file(tmp.path(), "c.rs", "use std::fs;\n"),
    ];

    let parsed = parse_grep_query("use std *.rs");
    let result = grep_search(
        &files,
        &parsed,
        &fuzzy_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    // Should only search .rs files
    for file in &result.files {
        assert!(
            file.relative_path().ends_with(".rs"),
            "should only match .rs files, got: {}",
            file.relative_path()
        );
    }
}

#[test]
fn fuzzy_respects_page_limit() {
    let tmp = TempDir::new().unwrap();
    let mut content = String::new();
    for i in 0..100 {
        content.push_str(&format!("line {} target\n", i));
    }
    let files = vec![create_file(tmp.path(), "big.txt", &content)];

    let mut opts = fuzzy_opts();
    opts.page_limit = 10;
    opts.max_matches_per_file = 50;

    let parsed = parse_grep_query("target");
    let result = grep_search(
        &files,
        &parsed,
        &opts,
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    // page_limit is a soft minimum: we always finish the current file, so we
    // get at least page_limit matches (no data loss) and at most
    // max_matches_per_file (200) from a single file.
    assert!(
        result.matches.len() >= opts.page_limit,
        "should return at least page_limit matches: got {}",
        result.matches.len()
    );
    assert!(
        result.matches.len() <= opts.max_matches_per_file,
        "should never exceed max_matches_per_file: got {}",
        result.matches.len()
    );

    assert_eq!(
        result.matches.len(),
        opts.max_matches_per_file,
        "all limit of lines must be returned"
    );
}

#[test]
fn fuzzy_respects_max_matches_per_file() {
    let tmp = TempDir::new().unwrap();
    let mut content = String::new();
    for i in 0..50 {
        content.push_str(&format!("line {} match_target\n", i));
    }
    let files = vec![create_file(tmp.path(), "many.txt", &content)];

    let mut opts = fuzzy_opts();
    opts.max_matches_per_file = 5;

    let parsed = parse_grep_query("match");
    let result = grep_search(
        &files,
        &parsed,
        &opts,
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        5,
        "should cap at max_matches_per_file"
    );
}

#[test]
fn fuzzy_filters_low_quality_matches() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "code.rs",
        "fn mutex_lock() {}\nfn xyz() {}\nfn abc_def_ghi() {}\nfn abcdefghij() {}\n",
    )];

    // Search for "abc" - should match "abc_def_ghi" and "abcdefghij" with high scores,
    // but NOT "xyz" (no relation) or "mutex_lock" (only weak letter overlap)
    let parsed = parse_grep_query("abc");
    let result = grep_search(
        &files,
        &parsed,
        &fuzzy_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    // Should only get high-quality matches
    assert!(
        result.matches.len() <= 2,
        "should filter out low-quality fuzzy matches, got {} matches",
        result.matches.len()
    );

    // All matches should contain reasonable character overlap
    for m in &result.matches {
        assert!(
            m.line_content.contains("abc") || m.line_content.contains("abc_"),
            "match '{}' should be high-quality",
            m.line_content
        );
    }
}

#[test]
fn fuzzy_exact_match_always_passes() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "test.txt",
        "exact match line\nno match here\n",
    )];

    // Exact matches should always pass regardless of score threshold
    let parsed = parse_grep_query("exact");
    let result = grep_search(
        &files,
        &parsed,
        &fuzzy_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        1,
        "exact match should always pass score threshold"
    );
    assert!(result.matches[0].line_content.contains("exact"));
}

#[test]
fn fuzzy_score_is_captured() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "test.txt",
        "hello world\ngoodbye world\n",
    )];

    let parsed = parse_grep_query("hello");
    let result = grep_search(
        &files,
        &parsed,
        &fuzzy_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 1);
    let m = &result.matches[0];

    // Fuzzy score should be set (Some) for fuzzy mode matches
    assert!(
        m.fuzzy_score.is_some(),
        "fuzzy_score should be set in fuzzy grep mode"
    );
    assert!(
        m.fuzzy_score.unwrap() > 0,
        "fuzzy_score should be positive for a good match"
    );
}

#[test]
fn fuzzy_score_is_none_in_plain_mode() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(tmp.path(), "test.txt", "hello world\n")];

    let parsed = parse_grep_query("hello");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(result.matches.len(), 1);
    let m = &result.matches[0];

    // fuzzy_score should be None for plain text mode
    assert!(
        m.fuzzy_score.is_none(),
        "fuzzy_score should be None in plain text mode"
    );
}

/// Regression: memmem prefilter rejected files where content casing differed
/// from the query, even under smart_case. E.g. "vfio-kvm" failed to find
/// "VFIO-KVM" because the lowercased finder did a case-sensitive scan.
#[test]
fn plain_text_smart_case_finds_uppercase_content_with_lowercase_query() {
    let tmp = TempDir::new().unwrap();
    let files = vec![create_file(
        tmp.path(),
        "driver.c",
        "// VFIO-KVM integration\nstatic int init(void) {}\n",
    )];

    let parsed = parse_grep_query("vfio-kvm");
    let result = grep_search(
        &files,
        &parsed,
        &plain_opts(),
        &ContentCacheBudget::unlimited(),
        None,
        None,
        None,
    );

    assert_eq!(
        result.matches.len(),
        1,
        "lowercase query should case-insensitively match 'VFIO-KVM'"
    );
}
