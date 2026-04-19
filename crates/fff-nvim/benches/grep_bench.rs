use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use fff::types::{ContentCacheBudget, FileItem};
use fff::{BigramFilter, GrepMode, GrepSearchOptions, build_bigram_index, grep};
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

struct TestData {
    files: Vec<FileItem>,
    bigram: BigramFilter,
    budget: ContentCacheBudget,
}

static SETUP: OnceLock<TestData> = OnceLock::new();

fn big_repo_path() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("BIG_REPO_PATH") {
        return std::path::PathBuf::from(path);
    }

    let candidates = [
        std::path::PathBuf::from("./big-repo"),
        std::path::PathBuf::from("../../big-repo"),
    ];
    for p in &candidates {
        if p.exists() {
            return p.clone();
        }
    }
    panic!(
        "./big-repo not found. Run from workspace root:\n  \
         git clone --depth 1 https://github.com/torvalds/linux.git big-repo"
    );
}

fn setup() -> &'static TestData {
    SETUP.get_or_init(|| {
        let repo = big_repo_path();
        let canonical = fff::path_utils::canonicalize(&repo).expect("canonicalize");

        eprintln!("Loading files from {:?}...", canonical);
        let mut files = load_files(&canonical);
        let budget = ContentCacheBudget::new_for_repo(files.len());

        // Warm the content cache so warm benchmarks hit OnceLock.
        // Use unlimited budget for warmup — we want ALL files cached.
        // The repo budget (5k cap for 93k files) would leave most uncached.
        eprintln!("Warming content cache for {} files...", files.len());
        {
            let warmup_budget = ContentCacheBudget::unlimited();
            let mut buf = Vec::with_capacity(64 * 1024);
            for f in files.iter() {
                let _ = f.get_content_for_search(&mut buf, &warmup_budget);
            }
        }

        eprintln!("Building bigram index...");
        let (bigram, binary_indices) = build_bigram_index(&files, &budget);
        for &i in &binary_indices {
            files[i].set_binary(true);
        }

        let non_binary = files.iter().filter(|f| !f.is_binary()).count();
        eprintln!(
            "Ready: {} files ({} non-binary), bigram {:.1} MB",
            files.len(),
            non_binary,
            bigram.heap_bytes() as f64 / (1024.0 * 1024.0),
        );

        TestData {
            files,
            bigram,
            budget,
        }
    })
}

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
            let size = entry.metadata().ok().map_or(0, |m| m.len());
            let is_binary = detect_binary(&path, size);

            let path_string = path.to_string_lossy().into_owned();
            let relative_start = (path_string.len() - relative_path.len()) as u16;
            let filename_start = path_string
                .rfind('/')
                .map(|i| i + 1)
                .unwrap_or(relative_start as usize) as u16;
            files.push(FileItem::new_raw(
                path_string,
                relative_start,
                filename_start,
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

fn plain_options() -> GrepSearchOptions {
    GrepSearchOptions {
        max_file_size: 10 * 1024 * 1024,
        max_matches_per_file: 200,
        smart_case: true,
        file_offset: 0,
        page_limit: 50,
        mode: GrepMode::PlainText,
        time_budget_ms: 0,
        before_context: 0,
        after_context: 0,
        classify_definitions: false,
        trim_whitespace: false,
    }
}

fn fuzzy_options() -> GrepSearchOptions {
    GrepSearchOptions {
        mode: GrepMode::Fuzzy,
        ..plain_options()
    }
}

fn do_grep(
    files: &[FileItem],
    query: &str,
    options: &GrepSearchOptions,
    budget: &ContentCacheBudget,
    bigram: Option<&BigramFilter>,
) -> usize {
    let parsed = grep::parse_grep_query(query);
    let result = grep::grep_search(
        black_box(files),
        black_box(&parsed),
        black_box(options),
        budget,
        bigram,
        None,
        None,
    );
    result.matches.len()
}

fn bench_plain_warm(c: &mut Criterion) {
    let test_picker = setup();
    let opts = plain_options();

    let queries: &[(&str, &str)] = &[
        ("2char_if", "if"),
        ("common_return", "return"),
        ("func_mutex_lock", "mutex_lock"),
        ("struct_inode_ops", "inode_operations"),
        ("define_MODULE_LICENSE", "MODULE_LICENSE"),
        ("rare_phylink_ethtool", "phylink_ethtool"),
        ("include", "#include"),
        ("comment_TODO", "TODO"),
        ("type_struct_file", "struct file"),
        ("error_EINVAL", "err = -EINVAL"),
        ("long_static_int_init", "static int __init"),
        ("very_common_int", "int"),
        ("single_char_x", "x"),
        ("path_printk_c", "printk *.c"),
        ("dir_mutex_kernel", "mutex /kernel/"),
    ];

    let mut group = c.benchmark_group("plain_warm");
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(5));

    for (name, query) in queries {
        group.bench_with_input(BenchmarkId::from_parameter(name), query, |b, q| {
            b.iter(|| do_grep(&test_picker.files, q, &opts, &test_picker.budget, None))
        });
    }

    group.finish();
}

fn bench_bigram_warm(c: &mut Criterion) {
    let test_picker = setup();
    let opts = plain_options();

    let queries: &[(&str, &str)] = &[
        ("2char_if", "if"),
        ("common_return", "return"),
        ("func_mutex_lock", "mutex_lock"),
        ("struct_inode_ops", "inode_operations"),
        ("define_MODULE_LICENSE", "MODULE_LICENSE"),
        ("rare_phylink_ethtool", "phylink_ethtool"),
        ("include", "#include"),
        ("comment_TODO", "TODO"),
        ("type_struct_file", "struct file"),
        ("error_EINVAL", "err = -EINVAL"),
        ("long_static_int_init", "static int __init"),
        ("very_common_int", "int"),
        ("single_char_x", "x"),
        ("path_printk_c", "printk *.c"),
        ("dir_mutex_kernel", "mutex /kernel/"),
    ];

    let mut group = c.benchmark_group("bigram_warm");
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(5));

    for (name, query) in queries {
        group.bench_with_input(BenchmarkId::from_parameter(name), query, |b, q| {
            b.iter(|| {
                do_grep(
                    &test_picker.files,
                    q,
                    &opts,
                    &test_picker.budget,
                    Some(&test_picker.bigram),
                )
            })
        });
    }

    group.finish();
}

fn bench_fuzzy_warm(c: &mut Criterion) {
    let test_picker = setup();
    let opts = fuzzy_options();

    let queries: &[(&str, &str)] = &[
        ("exact_mutex_lock", "mutex_lock"),
        ("typo_mutx_lock", "mutx_lock"),
        ("camel_InodeOps", "InodeOps"),
        ("abbrev_sched_rt", "sched_rt"),
        ("short_kfr", "kfr"),
        ("common_return", "return"),
        ("define_MODULE_LICENSE", "MODULE_LICENSE"),
        ("struct_file_ops", "file_operations"),
        ("long_static_int_init", "static_int_init"),
        ("path_printk_c", "printk *.c"),
    ];

    let mut group = c.benchmark_group("fuzzy_warm");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(8));

    for (name, query) in queries {
        group.bench_with_input(BenchmarkId::from_parameter(name), query, |b, q| {
            b.iter(|| do_grep(&test_picker.files, q, &opts, &test_picker.budget, None))
        });
    }

    group.finish();
}

fn bench_fuzzy_bigram_warm(c: &mut Criterion) {
    let test_picker = setup();
    let opts = fuzzy_options();

    let queries: &[(&str, &str)] = &[
        ("exact_mutex_lock", "mutex_lock"),
        ("typo_mutx_lock", "mutx_lock"),
        ("camel_InodeOps", "InodeOps"),
        ("abbrev_sched_rt", "sched_rt"),
        ("short_kfr", "kfr"),
        ("common_return", "return"),
        ("define_MODULE_LICENSE", "MODULE_LICENSE"),
        ("struct_file_ops", "file_operations"),
        ("long_static_int_init", "static_int_init"),
        ("path_printk_c", "printk *.c"),
    ];

    let mut group = c.benchmark_group("fuzzy_bigram_warm");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(8));

    for (name, query) in queries {
        group.bench_with_input(BenchmarkId::from_parameter(name), query, |b, q| {
            b.iter(|| {
                do_grep(
                    &test_picker.files,
                    q,
                    &opts,
                    &test_picker.budget,
                    Some(&test_picker.bigram),
                )
            })
        });
    }

    group.finish();
}

fn bench_plain_cold(c: &mut Criterion) {
    let test_picker = setup();
    let opts = plain_options();

    let queries: &[(&str, &str)] = &[
        ("2char_if", "if"),
        ("common_return", "return"),
        ("func_mutex_lock", "mutex_lock"),
        ("struct_inode_ops", "inode_operations"),
        ("define_MODULE_LICENSE", "MODULE_LICENSE"),
        ("rare_phylink_ethtool", "phylink_ethtool"),
        ("long_static_int_init", "static int __init"),
    ];

    let mut group = c.benchmark_group("plain_cold");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(10));

    let canonical = fff::path_utils::canonicalize(&big_repo_path()).expect("canonicalize");

    for (name, query) in queries {
        group.bench_with_input(BenchmarkId::from_parameter(name), query, |b, q| {
            b.iter_with_setup(
                || load_files(&canonical),
                |fresh_files| do_grep(&fresh_files, q, &opts, &test_picker.budget, None),
            );
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_plain_warm,
    bench_bigram_warm,
    bench_fuzzy_warm,
    bench_fuzzy_bigram_warm,
    bench_plain_cold,
);

criterion_main!(benches);
