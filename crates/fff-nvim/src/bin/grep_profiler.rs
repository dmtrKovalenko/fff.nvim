use fff_core::FileItem;
/// Live grep benchmark profiler for fff.nvim
///
/// Benchmarks the full grep pipeline against a large repository (Linux kernel).
/// Measures cold-cache, warm-cache, and incremental typing latencies to simulate
/// real user interaction patterns.
///
/// Uses direct WalkBuilder scanning (no background thread) for faster startup.
///
/// Usage:
///   cargo build --release --bin grep_profiler
///   ./target/release/grep_profiler [--path /path/to/repo]
use fff_core::grep::{GrepSearchOptions, grep_search, parse_grep_query};
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

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

struct BenchStats {
    times: Vec<Duration>,
}

impl BenchStats {
    fn new() -> Self {
        Self { times: Vec::new() }
    }

    fn push(&mut self, d: Duration) {
        self.times.push(d);
    }

    fn mean(&self) -> Duration {
        let total: Duration = self.times.iter().sum();
        total / self.times.len() as u32
    }

    fn median(&self) -> Duration {
        let mut sorted = self.times.clone();
        sorted.sort();
        sorted[sorted.len() / 2]
    }

    fn p95(&self) -> Duration {
        let mut sorted = self.times.clone();
        sorted.sort();
        let idx = ((sorted.len() as f64) * 0.95) as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    fn p99(&self) -> Duration {
        let mut sorted = self.times.clone();
        sorted.sort();
        let idx = ((sorted.len() as f64) * 0.99) as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    fn min(&self) -> Duration {
        *self.times.iter().min().unwrap()
    }

    fn max(&self) -> Duration {
        *self.times.iter().max().unwrap()
    }
}

struct GrepBench<'a> {
    files: &'a [FileItem],
    options: GrepSearchOptions,
}

impl<'a> GrepBench<'a> {
    fn new(files: &'a [FileItem]) -> Self {
        Self {
            files,
            options: GrepSearchOptions {
                max_file_size: 10 * 1024 * 1024,
                max_matches_per_file: 200,
                smart_case: true,
                file_offset: 0,
                page_limit: 50,
                mode: Default::default(),
                time_budget_ms: 0,
            },
        }
    }

    /// Run a single grep search, return (duration, match_count, files_searched)
    fn run_once(&self, query: &str) -> (Duration, usize, usize) {
        let parsed = parse_grep_query(query);
        let start = Instant::now();
        let result = grep_search(self.files, query, parsed, &self.options);
        let elapsed = start.elapsed();
        (elapsed, result.matches.len(), result.total_files_searched)
    }

    /// Benchmark a query with multiple iterations
    fn bench_query(&self, query: &str, iterations: usize) -> (BenchStats, usize, usize) {
        let mut stats = BenchStats::new();
        let mut last_matches = 0;
        let mut last_files_searched = 0;

        for _ in 0..iterations {
            let (elapsed, matches, files_searched) = self.run_once(query);
            stats.push(elapsed);
            last_matches = matches;
            last_files_searched = files_searched;
        }

        (stats, last_matches, last_files_searched)
    }
}

fn fmt_dur(d: Duration) -> String {
    let us = d.as_micros();
    if us > 1_000_000 {
        format!("{:.2}s", d.as_secs_f64())
    } else if us > 1000 {
        format!("{:.2}ms", us as f64 / 1000.0)
    } else {
        format!("{}us", us)
    }
}

fn print_row(name: &str, stats: &BenchStats, matches: usize, files_searched: usize, iters: usize) {
    eprintln!(
        "  {:<24} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8} | {:>6} | {:>6} | {:>4}",
        name,
        fmt_dur(stats.mean()),
        fmt_dur(stats.median()),
        fmt_dur(stats.p95()),
        fmt_dur(stats.p99()),
        fmt_dur(stats.min()),
        fmt_dur(stats.max()),
        matches,
        files_searched,
        iters,
    );
}

fn print_header() {
    eprintln!(
        "  {:<24} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8} | {:>8} | {:>6} | {:>6} | {:>4}",
        "Name", "Mean", "Median", "P95", "P99", "Min", "Max", "Match", "Files", "Iter"
    );
    eprintln!(
        "  {:-<24}-+-{:-<8}-+-{:-<8}-+-{:-<8}-+-{:-<8}-+-{:-<8}-+-{:-<8}-+-{:-<6}-+-{:-<6}-+-{:-<4}",
        "", "", "", "", "", "", "", "", "", ""
    );
}

fn main() {
    // Parse args
    let args: Vec<String> = std::env::args().collect();
    let repo_path = if let Some(idx) = args.iter().position(|a| a == "--path") {
        args.get(idx + 1)
            .map(|s| s.as_str())
            .unwrap_or("./big-repo")
    } else {
        "./big-repo"
    };

    let repo = std::path::PathBuf::from(repo_path);
    if !repo.exists() {
        eprintln!("Repository not found at: {}", repo_path);
        eprintln!("Usage: grep_profiler [--path /path/to/large/repo]");
        std::process::exit(1);
    }

    let canonical = fff_core::path_utils::canonicalize(&repo).expect("Failed to canonicalize path");
    eprintln!("=== FFF Live Grep Profiler ===");
    eprintln!("Repository: {:?}", canonical);

    // Direct file loading (no background thread)
    eprintln!("\n[1/5] Loading files...");
    let load_start = Instant::now();
    let files = load_files(&canonical);
    let load_time = load_start.elapsed();
    let non_binary = files.iter().filter(|f| !f.is_binary).count();
    let large_files = files.iter().filter(|f| f.size > 10 * 1024 * 1024).count();
    eprintln!(
        "  Loaded {} files in {:.2}s ({} non-binary, {} >10MB skipped)\n",
        files.len(),
        load_time.as_secs_f64(),
        non_binary,
        large_files,
    );

    let bench = GrepBench::new(&files);

    eprintln!("[2/5] Cold cache benchmarks (first search, mmap not yet loaded)");
    eprintln!("  Each query runs once with fresh FileItem mmaps.\n");
    print_header();

    let cold_queries: Vec<(&str, &str)> = vec![
        ("cold_common_2char", "if"),
        ("cold_common_word", "return"),
        ("cold_specific_func", "mutex_lock"),
        ("cold_struct_name", "inode_operations"),
        ("cold_define", "MODULE_LICENSE"),
        ("cold_rare_string", "phylink_ethtool"),
        ("cold_path_filter", "printk *.c"),
        ("cold_long_query", "static int __init"),
    ];

    for (name, query) in &cold_queries {
        // Re-load files to get fresh FileItems with no cached mmaps
        let fresh_files = load_files(&canonical);
        let fresh_bench = GrepBench::new(&fresh_files);
        let (stats, matches, files_searched) = fresh_bench.bench_query(query, 1);
        print_row(name, &stats, matches, files_searched, 1);
    }

    eprintln!("\n[3/5] Warm cache benchmarks (mmap cache populated)");
    eprintln!("  Running 3 warmup iterations, then measuring.\n");
    print_header();

    let warm_queries: Vec<(&str, &str, usize)> = vec![
        ("warm_2char", "if", 10),
        ("warm_common_word", "return", 10),
        ("warm_function_call", "mutex_lock", 15),
        ("warm_struct_name", "inode_operations", 15),
        ("warm_define", "MODULE_LICENSE", 15),
        ("warm_rare_string", "phylink_ethtool", 20),
        ("warm_include", "#include", 10),
        ("warm_comment", "TODO", 15),
        ("warm_type_decl", "struct file", 15),
        ("warm_error_path", "err = -EINVAL", 15),
        ("warm_long_pattern", "static int __init", 15),
        ("warm_very_common", "int", 10),
        ("warm_single_char", "x", 10),
        ("warm_path_constraint", "printk *.c", 15),
        ("warm_dir_constraint", "mutex /kernel/", 15),
    ];

    // Warmup pass - populate mmap cache
    for (_, query, _) in &warm_queries {
        for _ in 0..3 {
            bench.run_once(query);
        }
    }

    for (name, query, iters) in &warm_queries {
        let (stats, matches, files_searched) = bench.bench_query(query, *iters);
        print_row(name, &stats, matches, files_searched, *iters);
    }

    eprintln!("\n[4/5] Incremental typing simulation");
    eprintln!("  Simulates user typing character by character.\n");

    let typing_sequences: Vec<(&str, Vec<&str>)> = vec![
        (
            "mutex_lock",
            vec![
                "m",
                "mu",
                "mut",
                "mute",
                "mutex",
                "mutex_",
                "mutex_l",
                "mutex_lo",
                "mutex_loc",
                "mutex_lock",
            ],
        ),
        ("printk", vec!["p", "pr", "pri", "prin", "print", "printk"]),
        ("inode", vec!["i", "in", "ino", "inod", "inode"]),
        ("kfree", vec!["k", "kf", "kfr", "kfre", "kfree"]),
    ];

    for (name, sequence) in &typing_sequences {
        eprintln!("  Typing '{}' ({} keystrokes):", name, sequence.len());
        eprintln!(
            "    {:>16} | {:>8} | {:>6} | {:>6}",
            "Query", "Latency", "Match", "Files"
        );
        eprintln!("    {:-<16}-+-{:-<8}-+-{:-<6}-+-{:-<6}", "", "", "", "");

        for prefix in sequence {
            let (elapsed, matches, files_searched) = bench.run_once(prefix);
            eprintln!(
                "    {:>16} | {:>8} | {:>6} | {:>6}",
                format!("\"{}\"", prefix),
                fmt_dur(elapsed),
                matches,
                files_searched,
            );
        }
        eprintln!();
    }

    eprintln!("[5/5] Pagination benchmark");
    eprintln!("  Testing page_offset performance for common query.\n");

    let pagination_query = "return";
    eprintln!("  Query: \"{}\"", pagination_query);
    eprintln!(
        "    {:>6} | {:>12} | {:>8} | {:>6} | {:>12}",
        "Page", "File offset", "Latency", "Matches", "Next offset"
    );
    eprintln!(
        "    {:-<6}-+-{:-<12}-+-{:-<8}-+-{:-<6}-+-{:-<12}",
        "", "", "", "", ""
    );

    let mut file_offset = 0usize;
    for page in 0..10 {
        let parsed = parse_grep_query(pagination_query);
        let opts = GrepSearchOptions {
            max_file_size: 10 * 1024 * 1024,
            max_matches_per_file: 200,
            smart_case: true,
            file_offset,
            page_limit: 50,
            mode: Default::default(),
            time_budget_ms: 0,
        };
        let start = Instant::now();
        let result = grep_search(&files, pagination_query, parsed, &opts);
        let elapsed = start.elapsed();
        eprintln!(
            "    {:>6} | {:>12} | {:>8} | {:>6} | {:>12}",
            page,
            file_offset,
            fmt_dur(elapsed),
            result.matches.len(),
            result.next_file_offset,
        );

        if result.next_file_offset == 0 || result.matches.is_empty() {
            eprintln!("    (no more results)");
            break;
        }
        file_offset = result.next_file_offset;
    }

    eprintln!("\n=== Summary ===");
    let mmap_count = files.iter().filter(|f| f.get_mmap().is_some()).count();
    eprintln!("  Files with cached mmap: {}", mmap_count);
    eprintln!("  Total indexed files: {}", files.len());
    eprintln!("  Non-binary files: {}", non_binary);
    eprintln!("  Files > 10MB (skipped): {}", large_files);

    std::thread::sleep(Duration::from_millis(100));

    eprintln!("\nDone. For perf profiling:");
    eprintln!("  perf record -g --call-graph dwarf -F 999 ./target/release/grep_profiler");
    eprintln!("  perf report --no-children");
}
