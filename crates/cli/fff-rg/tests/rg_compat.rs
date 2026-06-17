#[path = "rg_compat/hay.rs"]
mod hay;
#[path = "rg_compat/synthetic.rs"]
mod synthetic;
#[path = "rg_compat/util.rs"]
mod util;

use std::process::Command;

use hay::{PROJECT, SHERLOCK};
use synthetic::{LARGE_REPO, MEDIUM_REPO, SMALL_REPO};
use test_case::test_case;
use util::{Dir, assert_rg_match, find_binary, normalize_inline};

#[test]
fn smoke_basic_search() {
    let dir = Dir::new("smoke");
    dir.create("sherlock", SHERLOCK);

    let out = dir.command().arg("--color=never").arg("--no-heading").arg("Sherlock").stdout();
    assert!(out.contains("Sherlock"), "expected Sherlock in output, got: {out}");
    let lines: Vec<&str> = out.lines().filter(|l| *l != "--").collect();
    assert_eq!(lines.len(), 2, "expected 2 matching lines, got {}: {out}", lines.len());
}


#[test]
fn case_insensitive() {
    let dir = Dir::new("case_i");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--no-heading", "-i", "sherlock"]).stdout();
    let lines: Vec<&str> = out.lines().filter(|l| *l != "--").collect();
    assert_eq!(lines.len(), 2, "expected 2 case-insensitive matches, got: {out}");
    assert_eq!(
        lines[0],
        "sherlock:For the Doctor Watsons of this world, as opposed to the Sherlock"
    );
    assert_eq!(
        lines[1],
        "sherlock:be, to a very large extent, the result of luck. Sherlock Holmes"
    );
}

#[test]
fn smart_case_lower() {
    let dir = Dir::new("smart_lower");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--no-heading", "sherlock"]).stdout();
    assert!(out.contains("Sherlock"), "smart case should match uppercase with lowercase query");
    let lines: Vec<&str> = out.lines().filter(|l| *l != "--").collect();
    assert_eq!(lines.len(), 2);
}

#[test]
fn smart_case_upper() {
    let dir = Dir::new("smart_upper");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--no-heading", "Sherlock"]).stdout();
    let lines: Vec<&str> = out.lines().filter(|l| *l != "--").collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("Sherlock"));
}

#[test]
fn case_sensitive() {
    let dir = Dir::new("case_s");
    dir.create("sherlock", SHERLOCK);
    let code = dir.command().args(&["--color=never", "--no-heading", "-s", "sherlock"]).exit_code();
    assert_eq!(code, 1, "case-sensitive 'sherlock' should find nothing");
}

#[test]
fn fixed_strings() {
    let dir = Dir::new("fixed");
    dir.create("test", "foo.bar\nfooXbar\n");
    let out = dir.command().args(&["--color=never", "--no-heading", "-F", "foo.bar"]).stdout();
    assert_eq!(out, "test:foo.bar\n");
}

#[test]
fn fixed_strings_regex_chars() {
    let dir = Dir::new("fixed_regex");
    dir.create("test", "a(b)c\nabc\n");
    let out = dir.command().args(&["--color=never", "--no-heading", "-F", "a(b)c"]).stdout();
    assert_eq!(out, "test:a(b)c\n");
}

#[test]
fn no_match_exit_code() {
    let dir = Dir::new("no_match");
    dir.create("sherlock", SHERLOCK);
    let code = dir.command().args(&["--color=never", "--no-heading", "ZZZZNOTFOUND"]).exit_code();
    assert_eq!(code, 1);
}

#[test]
fn match_exit_code() {
    let dir = Dir::new("match_exit");
    dir.create("sherlock", SHERLOCK);
    let code = dir.command().args(&["--color=never", "--no-heading", "Sherlock"]).exit_code();
    assert_eq!(code, 0);
}


#[test]
fn line_numbers() {
    let dir = Dir::new("line_num");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--no-heading", "-n", "Sherlock"]).stdout();
    let lines: Vec<&str> = out.lines().filter(|l| *l != "--").collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("sherlock:1:"));
    assert!(lines[1].starts_with("sherlock:3:"));
}

#[test]
fn column_numbers() {
    let dir = Dir::new("columns");
    dir.create("sherlock", SHERLOCK);
    let out = dir
        .command()
        .args(&["--color=never", "--no-heading", "-n", "--column", "Sherlock"])
        .stdout();
    let lines: Vec<&str> = out.lines().filter(|l| *l != "--").collect();
    assert_eq!(lines.len(), 2);
    // Format: file:line:col:content — at least 4 colon-separated parts
    let parts: Vec<&str> = lines[0].splitn(4, ':').collect();
    assert_eq!(parts.len(), 4, "expected file:line:col:content format");
    assert_eq!(parts[0], "sherlock");
}

#[test]
fn heading_mode() {
    let dir = Dir::new("heading");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--heading", "Sherlock"]).stdout();
    let lines: Vec<&str> = out.lines().filter(|l| *l != "--").collect();
    // First line should be just the filename (heading)
    assert_eq!(lines[0], "sherlock");
    // Match lines should NOT have filename prefix
    assert!(!lines[1].starts_with("sherlock:"));
    assert!(lines[1].contains("Sherlock"));
}

#[test]
fn heading_with_line_numbers() {
    let dir = Dir::new("heading_ln");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--heading", "-n", "Sherlock"]).stdout();
    let lines: Vec<&str> = out.lines().filter(|l| *l != "--").collect();
    assert_eq!(lines[0], "sherlock");
    assert!(lines[1].starts_with("1:"), "expected line number prefix, got: {}", lines[1]);
    assert!(lines[2].starts_with("3:"), "expected line number prefix, got: {}", lines[2]);
}

#[test]
fn no_filename() {
    let dir = Dir::new("no_filename");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--no-heading", "-I", "Sherlock"]).stdout();
    let lines: Vec<&str> = out.lines().filter(|l| *l != "--").collect();
    assert!(!lines[0].starts_with("sherlock:"), "should not have filename prefix");
    assert!(lines[0].contains("Sherlock"));
}

#[test]
fn count() {
    let dir = Dir::new("count");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--no-heading", "-c", "Sherlock"]).stdout();
    assert_eq!(out, "sherlock:2\n");
}

#[test]
fn files_with_matches() {
    let dir = Dir::new("files_match");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--no-heading", "-l", "Sherlock"]).stdout();
    assert_eq!(out, "sherlock\n");
}

#[test]
fn quiet_match() {
    let dir = Dir::new("quiet_match");
    dir.create("sherlock", SHERLOCK);
    let mut cmd = dir.command();
    cmd.args(&["--color=never", "--no-heading", "-q", "Sherlock"]);
    let out = cmd.stdout();
    assert!(out.is_empty(), "quiet mode should produce no output, got: {out}");
}

#[test]
fn quiet_no_match() {
    let dir = Dir::new("quiet_nomatch");
    dir.create("sherlock", SHERLOCK);
    let code =
        dir.command().args(&["--color=never", "--no-heading", "-q", "ZZZZNOTFOUND"]).exit_code();
    assert_eq!(code, 1);
}

#[test]
fn vimgrep() {
    let dir = Dir::new("vimgrep");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--vimgrep", "Sherlock"]).stdout();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "expected 2 vimgrep lines, got: {out}");
    for line in &lines {
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        assert_eq!(parts.len(), 4, "vimgrep format: file:line:col:content");
        assert_eq!(parts[0], "sherlock");
        assert!(parts[1].parse::<u64>().is_ok(), "line should be numeric");
        assert!(parts[2].parse::<u64>().is_ok(), "col should be numeric");
    }
}

#[test]
fn files_mode() {
    let dir = Dir::new("files_mode");
    dir.create("alpha.txt", "content");
    dir.create("beta.rs", "fn main() {}");
    let out = dir.command().args(&["--color=never", "--files"]).stdout();
    let mut lines: Vec<&str> = out.lines().collect();
    lines.sort();
    assert!(lines.contains(&"alpha.txt"), "should list alpha.txt, got: {out}");
    assert!(lines.contains(&"beta.rs"), "should list beta.rs, got: {out}");
}

/// Without context flags, non-adjacent matches should NOT have -- separator
#[test]
fn no_separator_without_context() {
    let dir = Dir::new("no_sep");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--no-heading", "Sherlock"]).stdout();
    assert!(!out.contains("--"), "should not emit -- separator without context flags, got: {out}");
}


#[test]
fn after_context() {
    let dir = Dir::new("after_ctx");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--no-heading", "-A1", "Sherlock"]).stdout();
    // Should have match lines + context lines
    assert!(out.contains("Holmeses"), "after-context should include next line");
    assert!(out.contains("can extract"), "after-context should include next line for 2nd match");
}

#[test]
fn after_context_line_numbers() {
    let dir = Dir::new("after_ctx_ln");
    dir.create("sherlock", SHERLOCK);
    let out =
        dir.command().args(&["--color=never", "--no-heading", "-A1", "-n", "Sherlock"]).stdout();
    // Match lines use ":" separator, context lines use "-" separator
    assert!(out.contains("sherlock:1:"), "should have match with line number");
    assert!(out.contains("sherlock-2-"), "context line should use - separator");
}

#[test]
fn before_context() {
    let dir = Dir::new("before_ctx");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--no-heading", "-B1", "Sherlock"]).stdout();
    // Line 3 matches; line 2 should be before-context
    assert!(out.contains("Holmeses"), "before-context of 2nd match should include line 2");
}

#[test]
fn before_context_line_numbers() {
    let dir = Dir::new("before_ctx_ln");
    dir.create("sherlock", SHERLOCK);
    let out =
        dir.command().args(&["--color=never", "--no-heading", "-B1", "-n", "Sherlock"]).stdout();
    assert!(out.contains("sherlock:1:"), "first match at line 1");
    assert!(out.contains("sherlock-2-"), "before-context for 2nd match");
    assert!(out.contains("sherlock:3:"), "second match at line 3");
}

#[test]
fn context_separator() {
    let dir = Dir::new("ctx_sep");
    dir.create("sherlock", SHERLOCK);
    // "world" is on line 1, "attached" is on line 6 — gap between them
    let out =
        dir.command().args(&["--color=never", "--no-heading", "-C1", "world|attached"]).stdout();
    assert!(out.contains("--"), "should have -- separator between non-adjacent groups");
}

#[test]
fn trim_whitespace() {
    let dir = Dir::new("trim");
    dir.create("indented", "    indented line\nnormal line\n");
    let out = dir.command().args(&["--color=never", "--no-heading", "--trim", "indented"]).stdout();
    // --trim strips leading whitespace
    assert!(out.contains("indented:indented line"), "should trim leading spaces, got: {out}");
    assert!(!out.contains("    indented"), "leading spaces should be stripped");
}

#[test]
fn max_count() {
    let dir = Dir::new("max_count");
    dir.create("sherlock", SHERLOCK);
    let out = dir.command().args(&["--color=never", "--no-heading", "-m1", "Sherlock"]).stdout();
    let lines: Vec<&str> = out.lines().filter(|l| *l != "--").collect();
    assert_eq!(lines.len(), 1, "max-count 1 should return 1 line, got: {out}");
    assert!(lines[0].contains("Sherlock"));
}

// --- rg comparison tests: project fixture ---

// inline mode
#[test_case(false, &["--color=never", "--no-heading", "fn"] ; "inline_basic")]
#[test_case(false, &["--color=never", "--no-heading", "-n", "fn"] ; "inline_line_numbers")]
#[test_case(false, &["--color=never", "--no-heading", "-n", "--column", "fn"] ; "inline_column")]
#[test_case(false, &["--color=never", "--no-heading", "-i", "config"] ; "inline_case_insensitive")]
#[test_case(false, &["--color=never", "--no-heading", "-s", "Config"] ; "inline_case_sensitive")]
#[test_case(false, &["--color=never", "--no-heading", "-S", "config"] ; "inline_smart_case_lower")]
#[test_case(false, &["--color=never", "--no-heading", "-S", "Config"] ; "inline_smart_case_upper")]
#[test_case(false, &["--color=never", "--no-heading", "-F", "HashMap"] ; "inline_fixed_strings")]
#[test_case(false, &["--color=never", "--no-heading", "-c", "fn"] ; "inline_count")]
#[test_case(false, &["--color=never", "--no-heading", "-l", "fn"] ; "inline_files_with_matches")]
#[test_case(false, &["--color=never", "--no-heading", "-m1", "fn"] ; "inline_max_count")]
#[test_case(false, &["--color=never", "--no-heading", "--trim", "let"] ; "inline_trim")]
// heading mode
#[test_case(true,  &["--color=never", "--heading", "fn"] ; "heading_basic")]
#[test_case(true,  &["--color=never", "--heading", "-n", "fn"] ; "heading_line_numbers")]
#[test_case(true,  &["--color=never", "--heading", "-n", "--column", "Config"] ; "heading_column")]
#[test_case(false, &["--color=never", "--heading", "-c", "fn"] ; "heading_count")]
#[test_case(true,  &["--color=never", "--heading", "-n", "-m1", "fn"] ; "heading_max_count")]
// context
#[test_case(false, &["--color=never", "--no-heading", "-n", "-A2", "fn main"] ; "after_context")]
#[test_case(false, &["--color=never", "--no-heading", "-n", "-B2", "fn main"] ; "before_context")]
#[test_case(false, &["--color=never", "--no-heading", "-n", "-C2", "HashMap"] ; "symmetric_context")]
#[test_case(false, &["--color=never", "--no-heading", "-n", "-B1", "-A3", "HashMap"] ; "asymmetric_context")]
#[test_case(true,  &["--color=never", "--heading", "-n", "-C1", "fn"] ; "context_heading")]
// vimgrep
#[test_case(false, &["--color=never", "--vimgrep", "Config"] ; "vimgrep_basic")]
#[test_case(false, &["--color=never", "--vimgrep", r"fn\s+\w+"] ; "vimgrep_regex")]
#[test_case(false, &["--color=never", "--vimgrep", "-F", "pub fn"] ; "vimgrep_fixed")]
#[test_case(false, &["--color=never", "--vimgrep", "-i", "hashmap"] ; "vimgrep_case_insensitive")]
// quiet / exit codes
#[test_case(false, &["--color=never", "-q", "fn"] ; "quiet_match")]
#[test_case(false, &["--color=never", "-q", "ZZZZZ_NEVER_MATCHES"] ; "quiet_no_match")]
#[test_case(false, &["--color=never", "-c", "ZZZZZ_NEVER_MATCHES"] ; "count_no_match")]
#[test_case(false, &["--color=never", "-l", "ZZZZZ_NEVER_MATCHES"] ; "files_with_matches_no_match")]
// regex
#[test_case(false, &["--color=never", "--no-heading", "HashMap|Config"] ; "regex_alternation")]
#[test_case(false, &["--color=never", "--no-heading", r"fn\s+\w+"] ; "regex_quantifier")]
#[test_case(false, &["--color=never", "--no-heading", "^use"] ; "regex_anchor")]
#[test_case(false, &["--color=never", "--no-heading", "assert[_!]"] ; "regex_char_class")]
#[test_case(false, &["--color=never", "--no-heading", "-F", "HashMap"] ; "fixed_special_chars")]
#[test_case(false, &["--color=never", "--no-heading", "-F", "Config::new"] ; "fixed_parens")]
// unicode
#[test_case(false, &["--color=never", "--no-heading", "café"] ; "unicode_latin_extended")]
#[test_case(false, &["--color=never", "--no-heading", "日本語"] ; "unicode_cjk")]
#[test_case(false, &["--color=never", "--no-heading", "-i", "prójéct"] ; "unicode_case_insensitive")]
#[test_case(false, &["--color=never", "--vimgrep", "café"] ; "unicode_vimgrep")]
// multi-flag combos
#[test_case(false, &["--color=never", "--no-heading", "--trim", "-n", "-C1", "HashMap"] ; "combo_trim_context_linenums")]
#[test_case(false, &["--color=never", "--no-heading", "-c", "-i", "self"] ; "combo_count_case_insensitive")]
#[test_case(false, &["--color=never", "--no-heading", "-n", "-m1", "-C1", "HashMap"] ; "combo_maxcount_context")]
#[test_case(true,  &["--color=never", "--heading", "-n", "--column", "-C1", "Config"] ; "combo_heading_context_column")]
#[test_case(false, &["--color=never", "--vimgrep", "-F", "-i", "verbose"] ; "combo_vimgrep_fixed_case")]
fn vs_rg_project(heading: bool, args: &[&str]) {
    let name = std::thread::current().name().unwrap().rsplit("::").next().unwrap().to_string();
    let dir = Dir::new(&name);
    dir.with_project(&PROJECT);
    assert_rg_match(&dir, args, heading);
}

// --- rg comparison tests: custom fixtures ---

#[test_case(false, &[("solo.txt", "hello world\nfoo bar\nhello again\n")], &["--color=never", "--no-heading", "-I", "-n", "hello", "solo.txt"] ; "inline_no_filename")]
// context
#[test_case(false, &[("dense.txt", "a\nMATCH\nb\nMATCH\nc\n")], &["--color=never", "--no-heading", "-n", "-H", "-C1", "MATCH", "dense.txt"] ; "context_overlapping")]
#[test_case(false, &[("edges.txt", "MATCH\na\nb\nc\nd\ne\nMATCH\n")], &["--color=never", "--no-heading", "-n", "-H", "-C2", "MATCH", "edges.txt"] ; "context_at_boundaries")]
#[test_case(false, &[("distant.txt", "MATCH\na\nb\nc\nd\ne\nf\nMATCH\n")], &["--color=never", "--no-heading", "-n", "-H", "-C1", "MATCH", "distant.txt"] ; "context_separator")]
// color
#[test_case(false, &[("data.txt", "hello world\nfoo bar\nhello again\n")], &["--color=always", "--no-heading", "-n", "-H", "hello", "data.txt"] ; "color_inline")]
#[test_case(true,  &[("data.txt", "hello world\nfoo bar\nhello again\n")], &["--color=always", "--heading", "-n", "-H", "hello", "data.txt"] ; "color_heading")]
#[test_case(false, &[("data.txt", "hello world\n")], &["--color=always", "--no-heading", "-n", "--column", "-H", "hello", "data.txt"] ; "color_column")]
#[test_case(false, &[("data.txt", "hello\nhello\n")], &["--color=always", "--no-heading", "-c", "-H", "hello", "data.txt"] ; "color_count")]
#[test_case(false, &[("data.txt", "hello\n")], &["--color=always", "-l", "-H", "hello", "data.txt"] ; "color_files_with_matches")]
// edge cases
#[test_case(false, &[("empty.txt", ""), ("notempty.txt", "hello\n")], &["--color=never", "--no-heading", "hello"] ; "empty_file")]
#[test_case(false, &[("data.txt", "hello world")], &["--color=never", "--no-heading", "-H", "hello", "data.txt"] ; "no_trailing_newline")]
#[test_case(false, &[("one.txt", "single line\n")], &["--color=never", "--no-heading", "-n", "-H", "-C2", "single", "one.txt"] ; "single_line_file")]
#[test_case(false, &[("a/b/c/d/deep.txt", "hello from the deep\n"), ("shallow.txt", "hello from shallow\n")], &["--color=never", "--no-heading", "hello"] ; "deeply_nested")]
fn vs_rg_fixture(heading: bool, files: &[(&str, &str)], args: &[&str]) {
    let name = std::thread::current().name().unwrap().rsplit("::").next().unwrap().to_string();
    let dir = Dir::new(&name);
    for (path, content) in files {
        dir.create(path, content);
    }
    assert_rg_match(&dir, args, heading);
}

// --- exit code tests (custom assertions, not macro-able) ---

#[test]
fn vs_rg_exit_code_match() {
    let dir = Dir::new("vs_exit_match");
    dir.with_project(&PROJECT);
    let fff = dir.command().args(&["--color=never", "fn"]).full_output();
    let rg = dir.rg().args(&["--color=never", "fn"]).full_output();
    assert_eq!(fff.code, 0, "fff-rg should exit 0 on match");
    assert_eq!(rg.code, 0, "rg should exit 0 on match");
}

#[test]
fn vs_rg_exit_code_no_match() {
    let dir = Dir::new("vs_exit_nomatch");
    dir.with_project(&PROJECT);
    let fff = dir.command().args(&["--color=never", "ZZZZZ"]).full_output();
    let rg = dir.rg().args(&["--color=never", "ZZZZZ"]).full_output();
    assert_eq!(fff.code, 1, "fff-rg should exit 1 on no match");
    assert_eq!(rg.code, 1, "rg should exit 1 on no match");
}

// --- session reuse: warm index consistency ---

#[test]
fn session_reuse_consistent_results() {
    let dir = Dir::new("session_reuse");
    dir.with_project(&PROJECT);
    let args = &["--color=never", "--no-heading", "-n", "fn"];

    let out1 = dir.command().args(args).full_output();
    let out2 = dir.command().args(args).full_output();
    let out3 = dir.command().args(args).full_output();

    assert_eq!(out1.code, out2.code);
    assert_eq!(out2.code, out3.code);

    let n1 = normalize_inline(&out1.stdout);
    let n2 = normalize_inline(&out2.stdout);
    let n3 = normalize_inline(&out3.stdout);
    assert_eq!(n1, n2, "warm index should return same results as cold");
    assert_eq!(n2, n3);
}

#[test]
fn session_reuse_different_queries() {
    let dir = Dir::new("session_diff_q");
    dir.with_project(&PROJECT);

    let out_fn = dir.command().args(&["--color=never", "--no-heading", "fn"]).full_output();
    let out_config = dir.command().args(&["--color=never", "--no-heading", "Config"]).full_output();
    let out_none = dir.command().args(&["--color=never", "--no-heading", "ZZZZNOTFOUND"]).full_output();

    assert_eq!(out_fn.code, 0);
    assert_eq!(out_config.code, 0);
    assert_eq!(out_none.code, 1);
    assert!(out_fn.stdout.contains("fn"));
    assert!(out_config.stdout.contains("Config"));
    assert!(out_none.stdout.is_empty());
}

#[test]
fn session_reuse_alternating_modes() {
    let dir = Dir::new("session_modes");
    dir.with_project(&PROJECT);

    let grep1 = dir.command().args(&["--color=never", "--no-heading", "fn"]).full_output();
    let files = dir.command().args(&["--color=never", "--files"]).full_output();
    let grep2 = dir.command().args(&["--color=never", "--no-heading", "fn"]).full_output();

    assert_eq!(grep1.code, 0);
    assert_eq!(files.code, 0);
    assert_eq!(grep2.code, 0);
    assert_eq!(
        normalize_inline(&grep1.stdout),
        normalize_inline(&grep2.stdout),
        "grep results should be stable across interleaved files queries"
    );
}

// --- concurrency: parallel searches ---

#[test]
fn concurrent_searches_no_corruption() {
    let dir = Dir::new("concurrent");
    dir.with_project(&PROJECT);
    let dir_path = dir.dir.clone();

    let handles: Vec<_> = (0..8)
        .map(|i| {
            let path = dir_path.clone();
            std::thread::spawn(move || {
                let bin = find_binary("fff-rg");
                let output = Command::new(&bin)
                    .current_dir(&path)
                    .args(["--color=never", "--no-heading", "-n", "fn"])
                    .output()
                    .unwrap();
                let stdout = String::from_utf8(output.stdout).unwrap();
                let code = output.status.code().unwrap_or(-1);
                (i, stdout, code)
            })
        })
        .collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    for (i, _, code) in &results {
        assert_eq!(*code, 0, "thread {i} got exit code {code}");
    }

    let normalized: Vec<String> = results.iter().map(|(_, out, _)| normalize_inline(out)).collect();
    for (i, norm) in normalized.iter().enumerate().skip(1) {
        assert_eq!(
            &normalized[0], norm,
            "thread {i} output differs from thread 0"
        );
    }
}

// --- files mode ---

#[test]
fn vs_rg_files_list() {
    let dir = Dir::new("vs_files_list");
    dir.with_project(&PROJECT);
    let fff = dir.command().args(&["--color=never", "--files"]).full_output();
    let rg = dir.rg().args(&["--color=never", "--files"]).full_output();

    assert_eq!(fff.code, rg.code, "exit code mismatch");

    let mut fff_lines: Vec<&str> = fff.stdout.lines().collect();
    let mut rg_lines: Vec<&str> = rg.stdout.lines().collect();
    fff_lines.sort();
    rg_lines.sort();
    assert_eq!(fff_lines, rg_lines, "file listings differ\nfff: {fff_lines:?}\nrg: {rg_lines:?}");
}

#[test]
fn files_mode_subdirectories() {
    let dir = Dir::new("files_subdirs");
    dir.with_project(&PROJECT);
    let out = dir.command().args(&["--color=never", "--files"]).full_output();
    assert_eq!(out.code, 0);
    let files: Vec<&str> = out.stdout.lines().collect();
    assert!(files.iter().any(|f| f.contains("src/")), "should find files in src/, got: {files:?}");
    assert!(files.iter().any(|f| f.contains("tests/")), "should find files in tests/, got: {files:?}");
    assert!(files.iter().any(|f| f.contains("data/")), "should find files in data/, got: {files:?}");
}

#[test]
fn files_mode_quiet() {
    let dir = Dir::new("files_quiet");
    dir.with_project(&PROJECT);
    let out = dir.command().args(&["--color=never", "--files", "-q"]).full_output();
    assert!(out.stdout.is_empty(), "quiet files should produce no output");
    assert_eq!(out.code, 0);
}

// --- synthetic repo: scale tests ---

#[test_case(&SMALL_REPO  ; "small_50_files")]
#[test_case(&MEDIUM_REPO ; "medium_200_files")]
#[test_case(&LARGE_REPO  ; "large_500_files")]
fn scale_unique_needle_finds_one_file(repo: &synthetic::SyntheticRepo) {
    let name = std::thread::current().name().unwrap().rsplit("::").next().unwrap().to_string();
    let dir = Dir::new(&name);
    let specs = repo.populate(&dir);

    let target = &specs[0];
    let out = dir
        .command()
        .args(&["--color=never", "--no-heading", "-l", &target.unique_needle])
        .full_output();

    assert_eq!(out.code, 0);
    let files: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(files.len(), 1, "unique needle should match exactly 1 file, got: {files:?}");
    assert!(files[0].contains(&target.path), "expected {}, got {}", target.path, files[0]);
}

#[test_case(&SMALL_REPO  ; "small_50_files")]
#[test_case(&MEDIUM_REPO ; "medium_200_files")]
#[test_case(&LARGE_REPO  ; "large_500_files")]
fn scale_common_needle_match_count(repo: &synthetic::SyntheticRepo) {
    let name = std::thread::current().name().unwrap().rsplit("::").next().unwrap().to_string();
    let dir = Dir::new(&name);
    let specs = repo.populate(&dir);

    let out = dir
        .command()
        .args(&["--color=never", "--no-heading", "-l", repo.common_needle])
        .full_output();

    assert_eq!(out.code, 0);
    let matched_files: Vec<&str> = out.stdout.lines().collect();
    let expected = specs.iter().filter(|s| s.has_common).count();
    assert_eq!(
        matched_files.len(),
        expected,
        "expected {expected} files with common needle, got {}",
        matched_files.len()
    );
}

#[test_case(&SMALL_REPO  ; "small_50_files")]
#[test_case(&MEDIUM_REPO ; "medium_200_files")]
#[test_case(&LARGE_REPO  ; "large_500_files")]
fn scale_count_mode_totals(repo: &synthetic::SyntheticRepo) {
    let name = std::thread::current().name().unwrap().rsplit("::").next().unwrap().to_string();
    let dir = Dir::new(&name);
    let specs = repo.populate(&dir);

    let out = dir
        .command()
        .args(&["--color=never", "--no-heading", "-c", repo.common_needle])
        .full_output();

    assert_eq!(out.code, 0);
    // Each file with the common needle has it exactly once
    let total: usize = out
        .stdout
        .lines()
        .filter_map(|l| l.rsplit(':').next()?.parse::<usize>().ok())
        .sum();
    let expected = specs.iter().filter(|s| s.has_common).count();
    assert_eq!(total, expected, "total count mismatch");
}

#[test_case(&SMALL_REPO  ; "small_50_files")]
#[test_case(&MEDIUM_REPO ; "medium_200_files")]
fn scale_line_numbers_correct(repo: &synthetic::SyntheticRepo) {
    let name = std::thread::current().name().unwrap().rsplit("::").next().unwrap().to_string();
    let dir = Dir::new(&name);
    let specs = repo.populate(&dir);

    let target = &specs[0];
    let out = dir
        .command()
        .args(&["--color=never", "--no-heading", "-n", &target.unique_needle])
        .full_output();

    assert_eq!(out.code, 0);
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(lines.len(), 1);
    // Format: path:linenum:content
    let parts: Vec<&str> = lines[0].splitn(3, ':').collect();
    assert_eq!(parts.len(), 3, "expected path:line:content, got: {}", lines[0]);
    let line_num: u64 = parts[1].parse().unwrap_or_else(|_| panic!("bad line num: {}", parts[1]));
    assert_eq!(line_num, target.unique_line, "line number mismatch for unique needle");
}

#[test_case(&SMALL_REPO  ; "small_50_files")]
#[test_case(&MEDIUM_REPO ; "medium_200_files")]
fn scale_heading_mode(repo: &synthetic::SyntheticRepo) {
    let name = std::thread::current().name().unwrap().rsplit("::").next().unwrap().to_string();
    let dir = Dir::new(&name);
    let specs = repo.populate(&dir);

    let out = dir
        .command()
        .args(&["--color=never", "--heading", "-n", repo.common_needle])
        .full_output();

    assert_eq!(out.code, 0);
    // In heading mode, file paths appear as standalone lines (no colon-separated content)
    let expected_files = specs.iter().filter(|s| s.has_common).count();
    // Split on double newline to get blocks, each block is one file
    let blocks: Vec<&str> = out.stdout.split("\n\n").filter(|b| !b.trim().is_empty()).collect();
    assert_eq!(
        blocks.len(),
        expected_files,
        "expected {expected_files} heading blocks, got {}",
        blocks.len()
    );
}

#[test_case(&SMALL_REPO  ; "small_50_files")]
#[test_case(&MEDIUM_REPO ; "medium_200_files")]
fn scale_vimgrep_format(repo: &synthetic::SyntheticRepo) {
    let name = std::thread::current().name().unwrap().rsplit("::").next().unwrap().to_string();
    let dir = Dir::new(&name);
    let specs = repo.populate(&dir);

    let out = dir
        .command()
        .args(&["--color=never", "--vimgrep", repo.common_needle])
        .full_output();

    assert_eq!(out.code, 0);
    let lines: Vec<&str> = out.stdout.lines().collect();
    let expected = specs.iter().filter(|s| s.has_common).count();
    assert_eq!(lines.len(), expected);

    for line in &lines {
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        assert_eq!(parts.len(), 4, "vimgrep format file:line:col:content, got: {line}");
        assert!(parts[1].parse::<u64>().is_ok(), "line should be numeric: {}", parts[1]);
        assert!(parts[2].parse::<u64>().is_ok(), "col should be numeric: {}", parts[2]);
        assert!(parts[3].contains(repo.common_needle));
    }
}

#[test_case(&SMALL_REPO  ; "small_50_files")]
#[test_case(&MEDIUM_REPO ; "medium_200_files")]
fn scale_no_match_exit_code(repo: &synthetic::SyntheticRepo) {
    let name = std::thread::current().name().unwrap().rsplit("::").next().unwrap().to_string();
    let dir = Dir::new(&name);
    repo.populate(&dir);

    let out = dir
        .command()
        .args(&["--color=never", "--no-heading", "ZZZZZ_ABSOLUTELY_NOT_IN_ANY_FILE"])
        .full_output();

    assert_eq!(out.code, 1, "no match should exit 1");
    assert!(out.stdout.is_empty());
}

#[test_case(&SMALL_REPO  ; "small_50_files")]
#[test_case(&MEDIUM_REPO ; "medium_200_files")]
fn scale_files_mode_lists_files(repo: &synthetic::SyntheticRepo) {
    let name = std::thread::current().name().unwrap().rsplit("::").next().unwrap().to_string();
    let dir = Dir::new(&name);
    repo.populate(&dir);

    let out = dir
        .command()
        .args(&["--color=never", "--files"])
        .full_output();

    assert_eq!(out.code, 0);
    let listed: Vec<&str> = out.stdout.lines().collect();
    // Daemon paginates file listings, so we just verify we get a non-trivial set
    assert!(
        listed.len() >= 10,
        "files mode should list files, got {}",
        listed.len()
    );
    // All listed paths should be valid relative paths
    for path in &listed {
        assert!(!path.is_empty(), "empty path in files listing");
        assert!(!path.starts_with('/'), "path should be relative: {path}");
    }
}

#[test_case(&SMALL_REPO  ; "small_50_files")]
#[test_case(&MEDIUM_REPO ; "medium_200_files")]
fn scale_context_output_has_context_lines(repo: &synthetic::SyntheticRepo) {
    let name = std::thread::current().name().unwrap().rsplit("::").next().unwrap().to_string();
    let dir = Dir::new(&name);
    repo.populate(&dir);

    let out = dir
        .command()
        .args(&["--color=never", "--no-heading", "-n", "-C1", repo.common_needle])
        .full_output();

    assert_eq!(out.code, 0);
    // Output should have more lines than match count (context adds surrounding lines)
    let match_lines = out.stdout.lines().filter(|l| l.contains(repo.common_needle)).count();
    let total_lines = out.stdout.lines().filter(|l| !l.is_empty() && *l != "--").count();
    assert!(
        total_lines > match_lines,
        "context mode should produce more lines than just matches: {total_lines} total, {match_lines} matches"
    );
    // Context lines use - as line number separator (vs : for match lines)
    let context_lines = out.stdout.lines().filter(|l| {
        !l.is_empty() && *l != "--" && !l.contains(repo.common_needle)
    }).count();
    assert!(context_lines > 0, "should have context lines around matches");
}

#[test_case(&SMALL_REPO  ; "small_50_files")]
#[test_case(&MEDIUM_REPO ; "medium_200_files")]
fn scale_color_output_has_ansi(repo: &synthetic::SyntheticRepo) {
    let name = std::thread::current().name().unwrap().rsplit("::").next().unwrap().to_string();
    let dir = Dir::new(&name);
    repo.populate(&dir);

    let out = dir
        .command()
        .args(&["--color=always", "--no-heading", "-n", repo.common_needle])
        .full_output();

    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("\x1b["), "color output should contain ANSI escapes");
    assert!(out.stdout.contains("\x1b[0m"), "should have RESET codes");
    assert!(out.stdout.contains("\x1b[1m\x1b[31m"), "should have RED_BOLD for match highlights");
}

#[test]
fn scale_concurrent_on_large_repo() {
    let dir = Dir::new("scale_concurrent");
    let specs = MEDIUM_REPO.populate(&dir);
    let dir_path = dir.dir.clone();
    let needle = MEDIUM_REPO.common_needle;
    let expected_files = specs.iter().filter(|s| s.has_common).count();

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let path = dir_path.clone();
            std::thread::spawn(move || {
                let bin = find_binary("fff-rg");
                let output = Command::new(&bin)
                    .current_dir(&path)
                    .args(["--color=never", "--no-heading", "-l", needle])
                    .output()
                    .unwrap();
                let stdout = String::from_utf8(output.stdout).unwrap();
                let code = output.status.code().unwrap_or(-1);
                (stdout, code)
            })
        })
        .collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    for (i, (stdout, code)) in results.iter().enumerate() {
        assert_eq!(*code, 0, "thread {i} exit code");
        let count = stdout.lines().count();
        assert_eq!(
            count, expected_files,
            "thread {i}: expected {expected_files} files, got {count}"
        );
    }
}

#[test]
fn scale_every_unique_needle_findable() {
    let dir = Dir::new("scale_all_needles");
    let specs = SMALL_REPO.populate(&dir);

    for spec in &specs {
        let out = dir
            .command()
            .args(&["--color=never", "--no-heading", "-c", &spec.unique_needle])
            .full_output();

        assert_eq!(out.code, 0, "needle {} should be found", spec.unique_needle);
        let total: usize = out
            .stdout
            .lines()
            .filter_map(|l| l.rsplit(':').next()?.parse::<usize>().ok())
            .sum();
        assert_eq!(total, 1, "needle {} should appear exactly once", spec.unique_needle);
    }
}
