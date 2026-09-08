use fff::file_picker::{FFFMode, FilePicker};
use fff::{FuzzySearchOptions, PaginationArgs, QueryParser, SharedFilePicker, SharedFrecency};
use std::time::{Duration, Instant};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// Runs at load time, before mimalloc maps its first arena.
#[used]
#[cfg_attr(
    any(target_os = "linux", target_os = "android"),
    unsafe(link_section = ".init_array")
)]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(windows, unsafe(link_section = ".CRT$XCU"))]
static TUNE_MIMALLOC: extern "C" fn() = fff::tune_mimalloc;

fn proc_status(key: &str) -> u64 {
    let content = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    content
        .lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

fn smaps(key: &str) -> u64 {
    let content = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
    content
        .lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

fn dump_smaps() {
    let content = std::fs::read_to_string("/proc/self/smaps").unwrap_or_default();
    let mut maps: Vec<(u64, String)> = Vec::new();
    let mut header = String::new();
    for line in content.lines() {
        // mapping header lines look like "addr-addr perms offset dev inode [path]"
        if !line.ends_with("kB") && line.split_whitespace().count() >= 5 {
            header = line.to_string();
        } else if let Some(rest) = line.strip_prefix("Rss:") {
            let kb: u64 = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            if kb >= 1024 {
                let cols: Vec<&str> = header.split_whitespace().collect();
                let range = cols.first().copied().unwrap_or("");
                let (a, b) = range.split_once('-').unwrap_or(("0", "0"));
                let size = u64::from_str_radix(b, 16)
                    .unwrap_or(0)
                    .saturating_sub(u64::from_str_radix(a, 16).unwrap_or(0));
                maps.push((
                    kb * 1024,
                    format!(
                        "size={:>9.2}MB perms={} {}",
                        mb(size),
                        cols.get(1).unwrap_or(&""),
                        cols.get(5).unwrap_or(&"")
                    ),
                ));
            }
        }
    }
    maps.sort_by_key(|m| std::cmp::Reverse(m.0));
    for (rss, desc) in maps.iter().take(14) {
        println!("      rss={:>8.2}MB {desc}", mb(*rss));
    }
}

fn report(stage: &str) {
    if std::env::var_os("FFF_BENCH_SMAPS").is_some() {
        dump_smaps();
    }
    let rss = proc_status("VmRSS:");
    let anon = smaps("Anonymous:");
    println!(
        "[{stage:<22}] rss={:>8.2}MB anon={:>8.2}MB file={:>8.2}MB thp={:>8.2}MB hwm={:>8.2}MB",
        mb(rss),
        mb(anon),
        mb(rss.saturating_sub(anon)),
        mb(smaps("AnonHugePages:")),
        mb(proc_status("VmHWM:"))
    );
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "big-repo".to_string());
    let path = fff::path_utils::canonicalize(std::path::Path::new(&path))
        .expect("repo path")
        .to_string_lossy()
        .into_owned();

    report("startup");
    let sp = SharedFilePicker::default();
    let sf = SharedFrecency::default();
    let start = Instant::now();
    FilePicker::new_with_shared_state(
        sp.clone(),
        sf.clone(),
        fff::FilePickerOptions {
            base_path: path,
            enable_content_indexing: true,
            mode: FFFMode::Neovim,
            watch: false,
            ..Default::default()
        },
    )
    .expect("init");

    sp.wait_for_scan(Duration::from_secs(300));
    let walk_time = start.elapsed();
    report("after walk");
    sp.wait_for_indexing_complete(Duration::from_secs(600));
    let index_time = start.elapsed();
    report("after post-scan");

    {
        let guard = sp.read().unwrap();
        let picker = guard.as_ref().unwrap();
        let files = picker.get_files();
        let dirs = picker.get_dirs();
        let (arena, _, _) = picker.arena_bytes();
        let file_item = std::mem::size_of::<fff::FileItem>();
        let dir_item = std::mem::size_of::<fff::DirItem>();
        println!(
            "files={} dirs={} walk={walk_time:?} index={index_time:?}",
            files.len(),
            dirs.len()
        );
        println!(
            "  FileItem={file_item}B files_vec={:.2}MB DirItem={dir_item}B dirs_vec={:.2}MB path_arena={:.2}MB",
            mb(std::mem::size_of_val(files) as u64),
            mb(std::mem::size_of_val(dirs) as u64),
            mb(arena as u64)
        );
        if let Some(bi) = picker.bigram_index() {
            let skip = bi.skip_index();
            println!(
                "  bigram total={:.2}MB consec: dense={} ({:.2}MB) sparse={} ({:.2}MB) words={} | skip: dense={} ({:.2}MB) sparse={} ({:.2}MB)",
                mb(bi.heap_bytes() as u64),
                bi.dense_count(),
                mb((bi.dense_data().len() * 8) as u64),
                bi.sparse_count(),
                mb(bi.sparse_bytes() as u64),
                bi.words(),
                skip.map_or(0, |s| s.dense_count()),
                mb(skip.map_or(0, |s| s.dense_data().len() * 8) as u64),
                skip.map_or(0, |s| s.sparse_count()),
                mb(skip.map_or(0, |s| s.sparse_bytes()) as u64),
            );
            for (name, idx) in [("consec", Some(bi)), ("skip", skip)] {
                let Some(idx) = idx else { continue };
                let words = idx.words();
                let populated = idx.populated().max(1) as f64;
                let mut hist = [0usize; 10];
                for c in 0..idx.dense_count() {
                    let col = &idx.dense_data()[c * words..(c + 1) * words];
                    let pop: u64 = col.iter().map(|w| w.count_ones() as u64).sum();
                    let bucket = ((pop as f64 / populated) * 10.0).min(9.0) as usize;
                    hist[bucket] += 1;
                }
                println!("  {name} density histogram (10% buckets): {hist:?}");
            }
        }
    }

    // search perf sanity
    let queries = [
        "main",
        "sched_rt",
        "mutex_lock",
        "drivers/net",
        "Kconfig",
        "fs/ext4/inode.c",
    ];
    let mut total = Duration::ZERO;
    let iters = 20;
    for _ in 0..iters {
        for q in queries {
            let guard = sp.read().unwrap();
            let picker = guard.as_ref().unwrap();
            let parser = QueryParser::default();
            let parsed = parser.parse(q);
            let t = Instant::now();
            let r = picker.fuzzy_search(
                &parsed,
                None,
                FuzzySearchOptions {
                    max_threads: 0,
                    pagination: PaginationArgs {
                        offset: 0,
                        limit: 50,
                    },
                    ..Default::default()
                },
            );
            total += t.elapsed();
            std::hint::black_box(r.total_matched);
        }
    }
    println!(
        "fuzzy avg={:.3}ms over {} searches",
        total.as_secs_f64() * 1000.0 / (iters * queries.len()) as f64,
        iters * queries.len()
    );

    let grep_queries = [
        "mutex_lock",
        "EXPORT_SYMBOL_GPL",
        "sched_rt_runtime",
        "xyzzy_no_match",
    ];
    for q in grep_queries {
        let guard = sp.read().unwrap();
        let picker = guard.as_ref().unwrap();
        let parser = QueryParser::default();
        let parsed = parser.parse(q);
        let opts = fff::GrepSearchOptions::default();
        let mut best = Duration::MAX;
        let mut sum = Duration::ZERO;
        let mut matches = 0;
        let mut searched = 0;
        for _ in 0..7 {
            let t = Instant::now();
            let r = picker.grep(&parsed, &opts);
            let el = t.elapsed();
            best = best.min(el);
            sum += el;
            matches = r.matches.len();
            searched = r.total_files_searched;
        }
        println!(
            "grep {q:<20} min={:>7.2}ms avg={:>7.2}ms matches={matches} files_searched={searched}",
            best.as_secs_f64() * 1000.0,
            sum.as_secs_f64() * 1000.0 / 7.0
        );
    }
    report("after searches");
}
