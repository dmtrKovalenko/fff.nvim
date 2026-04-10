/// Single-query grep benchmark with bigram index profiling.
///
/// Usage:
///   cargo build --release --bin bench_grep_query
///   ./target/release/bench_grep_query --path ~/dev/chromium --query "MAX_FILE_SIZE" --iters 3
///   ./target/release/bench_grep_query --path ~/dev/chromium --query "TODO" --no-bigram
use fff::grep::{GrepMode, GrepSearchOptions, grep_search, parse_grep_query};
use fff::types::ContentCacheBudget;
use std::time::Instant;

fn fmt_dur(us: u128) -> String {
    if us > 1_000_000 {
        format!("{:.2}s", us as f64 / 1_000_000.0)
    } else if us > 1000 {
        format!("{:.2}ms", us as f64 / 1000.0)
    } else {
        format!("{}µs", us)
    }
}

fn run_grep(files: &[fff::FileItem], index: Option<&fff::BigramFilter>, query: &str, iters: usize) {
    let options = GrepSearchOptions {
        max_file_size: 10 * 1024 * 1024,
        max_matches_per_file: 200,
        smart_case: true,
        file_offset: 0,
        page_limit: usize::MAX,
        mode: GrepMode::PlainText,
        time_budget_ms: 0,
        before_context: 0,
        after_context: 0,
        classify_definitions: false,
        trim_whitespace: false,
    };

    let parsed = parse_grep_query(query);
    let budget = ContentCacheBudget::default();
    let mut times_us = Vec::with_capacity(iters);

    for i in 0..iters {
        let t = Instant::now();
        let result = grep_search(files, &parsed, &options, &budget, index, None, None);
        let us = t.elapsed().as_micros();
        times_us.push(us);

        eprintln!(
            "  iter {}: {} ({} matches in {} files, {}/{} searched)",
            i + 1,
            fmt_dur(us),
            result.matches.len(),
            result.files_with_matches,
            result.total_files_searched,
            result.total_files,
        );
    }

    if times_us.len() > 1 {
        times_us.sort();
        let sum: u128 = times_us.iter().sum();
        let mean = sum / times_us.len() as u128;
        let median = times_us[times_us.len() / 2];
        let min = times_us[0];
        let max = times_us[times_us.len() - 1];
        eprintln!(
            "  mean: {}  median: {}  min: {}  max: {}",
            fmt_dur(mean),
            fmt_dur(median),
            fmt_dur(min),
            fmt_dur(max)
        );
    }
}

fn build_bigram(files: &mut [fff::FileItem]) -> fff::BigramFilter {
    let budget = ContentCacheBudget::default();
    let (index, binary_indices) = fff::build_bigram_index(files, &budget);

    for &i in &binary_indices {
        files[i].set_binary(true);
    }

    index
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let path = args
        .iter()
        .position(|a| a == "--path")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or(".");

    let query = args
        .iter()
        .position(|a| a == "--query")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("TODO");

    let iters: usize = args
        .iter()
        .position(|a| a == "--iters")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let no_bigram = args.iter().any(|a| a == "--no-bigram");

    let repo = std::path::PathBuf::from(path);
    if !repo.exists() {
        eprintln!("Path not found: {}", path);
        eprintln!("Usage: bench_grep_query --path <dir> --query <text> [--iters N] [--no-bigram]");
        std::process::exit(1);
    }

    let canonical = fff::path_utils::canonicalize(&repo).expect("Failed to canonicalize path");
    eprintln!("=== bench_grep_query ===");
    eprintln!("Path:  {}", canonical.display());
    eprintln!("Query: \"{}\"", query);
    eprintln!("Iters: {}", iters);
    eprintln!();

    // ── 1. Scan files ──────────────────────────────────────────────────
    eprint!("[1/3] Scanning files... ");
    let t = Instant::now();
    let mut files = fff::scan_files(&canonical);
    let non_binary = files.iter().filter(|f| !f.is_binary()).count();
    eprintln!(
        "{} files in {:.2}s ({} non-binary)",
        files.len(),
        t.elapsed().as_secs_f64(),
        non_binary,
    );

    if no_bigram {
        eprintln!("[2/3] Bigram index skipped (--no-bigram)");
        eprintln!(
            "\n[3/3] Running grep \"{}\" x {} iterations\n",
            query, iters
        );
        run_grep(&files, None, query, iters);
        return;
    }

    // ── 2. Build bigram index ──────────────────────────────────────────
    eprint!("[2/3] Bigram index... ");
    let t = Instant::now();
    let index = build_bigram(&mut files);
    eprintln!(
        "done in {:.2}s  ({} cols, {:.1} MB)",
        t.elapsed().as_secs_f64(),
        index.columns_used(),
        index.heap_bytes() as f64 / (1024.0 * 1024.0),
    );

    // ── 3. Grep ───────────────────────────────────────────────────────
    eprintln!(
        "\n[3/3] Running grep \"{}\" x {} iterations\n",
        query, iters
    );
    run_grep(&files, Some(&index), query, iters);
}
