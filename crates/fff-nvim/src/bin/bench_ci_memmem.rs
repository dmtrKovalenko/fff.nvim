/// Benchmark: AVX2 vs scalar case-insensitive memmem prefilter.
///
/// Loads all non-binary file contents from a repo, then times both
/// implementations scanning every file for the query.
///
/// Usage:
///   cargo build --release --bin bench_ci_memmem
///   ./target/release/bench_ci_memmem --path ./big-repo --query "nomore" --iters 5
use fff::case_insensitive_memmem;
use std::io::Read;
use std::path::Path;
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

fn stats(times_us: &mut [u128]) -> (u128, u128, u128, u128) {
    times_us.sort();
    let sum: u128 = times_us.iter().sum();
    let mean = sum / times_us.len() as u128;
    let median = times_us[times_us.len() / 2];
    (mean, median, times_us[0], times_us[times_us.len() - 1])
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

fn load_file_contents(base_path: &Path) -> Vec<Vec<u8>> {
    use ignore::WalkBuilder;

    let mut contents = Vec::new();
    let max_size = 10 * 1024 * 1024u64;

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
            let path = entry.path();
            let size = entry.metadata().ok().map_or(0, |m| m.len());
            if size == 0 || size > max_size || detect_binary(path, size) {
                return;
            }
            if let Ok(data) = std::fs::read(path) {
                contents.push(data);
            }
        });

    contents
}

fn bench_impl(
    label: &str,
    contents: &[Vec<u8>],
    needle_lower: &[u8],
    total_bytes: u64,
    iters: usize,
    search_fn: fn(&[u8], &[u8]) -> bool,
) {
    eprintln!("\n  [{}]", label);
    let mut times = Vec::with_capacity(iters);
    let mut hit_count = 0u32;

    for i in 0..iters {
        let t = Instant::now();
        let mut hits = 0u32;
        for content in contents {
            if search_fn(content, needle_lower) {
                hits += 1;
            }
        }
        let us = t.elapsed().as_micros();
        times.push(us);
        hit_count = hits;
        let tp = total_bytes as f64 / (us as f64 / 1_000_000.0) / (1024.0 * 1024.0 * 1024.0);
        eprintln!(
            "    iter {}: {}  ({} hits, {:.2} GB/s)",
            i + 1,
            fmt_dur(us),
            hits,
            tp
        );
    }

    let (mean, median, min, max) = stats(&mut times);
    let med_tp = total_bytes as f64 / (median as f64 / 1_000_000.0) / (1024.0 * 1024.0 * 1024.0);
    eprintln!(
        "    mean: {}  median: {} ({:.2} GB/s)  min: {}  max: {}  hits: {}",
        fmt_dur(mean),
        fmt_dur(median),
        med_tp,
        fmt_dur(min),
        fmt_dur(max),
        hit_count
    );
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

    let repo = std::path::PathBuf::from(path);
    if !repo.exists() {
        eprintln!("Path not found: {}", path);
        eprintln!("Usage: bench_ci_memmem --path <dir> --query <text> [--iters N]");
        std::process::exit(1);
    }

    let canonical = fff::path_utils::canonicalize(&repo).expect("Failed to canonicalize path");
    let needle_lower: Vec<u8> = query.bytes().map(|b| b.to_ascii_lowercase()).collect();

    eprintln!("=== bench_ci_memmem: AVX2 vs Scalar ===");
    eprintln!("Path:   {}", canonical.display());
    eprintln!("Query:  \"{}\"", query);
    eprintln!("Needle: {:?}", std::str::from_utf8(&needle_lower).unwrap());
    eprintln!("Iters:  {}", iters);

    // ── Load all file contents into memory ─────────────────────────────
    eprint!("\n[1/2] Loading files into memory... ");
    let t = Instant::now();
    let contents = load_file_contents(&canonical);
    let total_bytes: u64 = contents.iter().map(|c| c.len() as u64).sum();
    eprintln!(
        "{} files, {:.1} MB in {:.2}s",
        contents.len(),
        total_bytes as f64 / (1024.0 * 1024.0),
        t.elapsed().as_secs_f64()
    );

    // ── Benchmark ──────────────────────────────────────────────────────
    eprintln!("\n[2/2] Benchmarking memmem prefilter (scanning ALL files)");

    bench_impl(
        "Packed pair (AVX2 two-byte scan)",
        &contents,
        &needle_lower,
        total_bytes,
        iters,
        case_insensitive_memmem::search_packed_pair,
    );

    bench_impl(
        "memchr2 first-byte + AVX2 verify",
        &contents,
        &needle_lower,
        total_bytes,
        iters,
        case_insensitive_memmem::search,
    );

    bench_impl(
        "memchr2 first-byte + scalar verify",
        &contents,
        &needle_lower,
        total_bytes,
        iters,
        case_insensitive_memmem::search_scalar,
    );
}
