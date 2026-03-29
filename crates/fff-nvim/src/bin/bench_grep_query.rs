/// Single-query grep benchmark with bigram index profiling.
///
/// Scans a repository, builds the bigram index, reports its size, then runs
/// a grep query for the requested number of iterations showing per-iteration
/// and aggregate timing.
///
/// Usage:
///   cargo build --release --bin bench_grep_query
///   ./target/release/bench_grep_query --path ~/dev/chromium --query "MAX_FILE_SIZE" --iters 3
use fff::FileItem;
use fff::grep::{GrepMode, GrepSearchOptions, grep_search, parse_grep_query};
use fff::types::{BigramIndexBuilder, ContentCacheBudget};
use std::io::Read;
use std::path::Path;
use std::time::Instant;

fn load_files(base_path: &Path) -> Vec<FileItem> {
    use ignore::WalkBuilder;

    let mut files = Vec::new();

    WalkBuilder::new(base_path)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .follow_links(false)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_some_and(|ft| ft.is_file()))
        .for_each(|entry| {
            let path = entry.path().to_path_buf();
            let relative = pathdiff::diff_paths(&path, base_path).unwrap_or_else(|| path.clone());
            let relative_path = relative.to_string_lossy().into_owned();
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let size = entry.metadata().ok().map_or(0, |m| m.len());
            let is_binary = detect_binary(&path, size);

            files.push(FileItem::new_raw(
                path,
                relative_path,
                file_name,
                size,
                0,
                None,
                is_binary,
            ));
        });

    files
}

fn detect_binary(path: &Path, size: u64) -> bool {
    if size == 0 {
        return false;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut reader = std::io::BufReader::with_capacity(1024, file);
    let mut buf = [0u8; 512];
    let n = reader.read(&mut buf).unwrap_or(0);
    buf[..n].contains(&0)
}

fn fmt_dur(us: u128) -> String {
    if us > 1_000_000 {
        format!("{:.2}s", us as f64 / 1_000_000.0)
    } else if us > 1000 {
        format!("{:.2}ms", us as f64 / 1000.0)
    } else {
        format!("{}µs", us)
    }
}

fn run_grep(
    files: &[FileItem],
    index: Option<&fff::types::BigramFilter>,
    query: &str,
    iters: usize,
) {
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
    };

    let parsed = parse_grep_query(query);
    let budget = ContentCacheBudget::default();
    let mut times_us = Vec::with_capacity(iters);

    for i in 0..iters {
        let t = Instant::now();
        let result = grep_search(files, &parsed, &options, &budget, index, None);
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
    let files = load_files(&canonical);
    let scan_time = t.elapsed();
    let non_binary = files.iter().filter(|f| !f.is_binary).count();
    eprintln!(
        "{} files in {:.2}s ({} non-binary)",
        files.len(),
        scan_time.as_secs_f64(),
        non_binary,
    );

    // ── 2. Build bigram index ──────────────────────────────────────────
    if no_bigram {
        eprintln!("[2/3] Bigram index skipped (--no-bigram)");
        eprintln!(
            "\n[3/3] Running grep \"{}\" x {} iterations\n",
            query, iters
        );
        run_grep(&files, None, query, iters);
        return;
    }

    use rayon::prelude::*;

    eprint!("[2/3] Building bigram index... ");
    let t = Instant::now();
    let budget = ContentCacheBudget::default();
    let builder = BigramIndexBuilder::new(files.len());
    let skip_builder = BigramIndexBuilder::new(files.len());

    files.par_iter().enumerate().for_each(|(idx, file)| {
        if !file.is_binary
            && let Some(content) = file.get_content_for_search(&budget)
        {
            builder.add_file_content(idx, &content);
            skip_builder.add_file_content_skip(idx, &content);
        }
    });

    let mut index = builder.compress();
    let skip_index = skip_builder.compress();
    let build_time = t.elapsed();
    eprintln!("done in {:.2}s", build_time.as_secs_f64());
    eprintln!(
        "       consecutive: {} cols, {:.2} MB",
        index.columns_used(),
        index.heap_bytes() as f64 / (1024.0 * 1024.0),
    );
    eprintln!(
        "       skip-1:      {} cols, {:.2} MB",
        skip_index.columns_used(),
        skip_index.heap_bytes() as f64 / (1024.0 * 1024.0),
    );
    index.set_skip_index(skip_index);
    eprintln!(
        "       total: {:.2} MB heap, {} files tracked",
        index.heap_bytes() as f64 / (1024.0 * 1024.0),
        index.file_count(),
    );

    // ── 3. Run grep query ──────────────────────────────────────────────
    eprintln!(
        "\n[3/3] Running grep \"{}\" x {} iterations\n",
        query, iters
    );
    run_grep(&files, Some(&index), query, iters);
}
