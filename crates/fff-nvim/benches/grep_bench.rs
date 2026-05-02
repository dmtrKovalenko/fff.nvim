use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use fff::file_picker::{FFFMode, FilePicker};
use fff::{
    FilePickerOptions, GrepMode, GrepSearchOptions, SharedFilePicker, SharedFrecency,
    parse_grep_query,
};
use std::sync::OnceLock;
use std::time::Duration;

struct TestData {
    shared_picker: SharedFilePicker,
}

static SETUP: OnceLock<TestData> = OnceLock::new();

fn big_repo_path() -> String {
    if let Some(path) = std::env::var_os("BIG_REPO_PATH") {
        return path.to_string_lossy().into_owned();
    }

    let candidates = ["./big-repo", "../../big-repo"];
    for p in &candidates {
        if std::path::Path::new(p).exists() {
            return p.to_string();
        }
    }
    panic!(
        "./big-repo not found. Run from workspace root:\n  \
         git clone --depth 1 https://github.com/torvalds/linux.git big-repo"
    );
}

fn setup() -> &'static TestData {
    SETUP.get_or_init(|| {
        let path = big_repo_path();
        let shared_picker = SharedFilePicker::default();
        let shared_frecency = SharedFrecency::default();

        eprintln!("Initializing FilePicker for {:?}...", path);
        FilePicker::new_with_shared_state(
            shared_picker.clone(),
            shared_frecency.clone(),
            FilePickerOptions {
                base_path: path,
                enable_mmap_cache: true,
                enable_content_indexing: true,
                mode: FFFMode::Neovim,
                ..Default::default()
            },
        )
        .expect("create picker");

        eprintln!("Waiting for scan completion...");
        shared_picker.wait_for_scan(Duration::from_secs(120));

        eprintln!("Waiting for warmup (bigram index)...");
        loop {
            let guard = shared_picker.read().expect("read lock");
            let picker = guard.as_ref().expect("picker present");
            let progress = picker.get_scan_progress();
            if progress.is_warmup_complete {
                let file_count = picker.get_files().len();
                eprintln!("Ready: {} files indexed, bigram built", file_count);
                break;
            }
            drop(guard);
            std::thread::sleep(Duration::from_millis(100));
        }

        TestData { shared_picker }
    })
}

fn setup_cold() -> SharedFilePicker {
    let path = big_repo_path();
    let shared_picker = SharedFilePicker::default();
    let shared_frecency = SharedFrecency::default();

    FilePicker::new_with_shared_state(
        shared_picker.clone(),
        shared_frecency.clone(),
        FilePickerOptions {
            base_path: path,
            enable_mmap_cache: false,
            enable_content_indexing: false,
            mode: FFFMode::Neovim,
            watch: false,
            ..Default::default()
        },
    )
    .expect("create picker");

    shared_picker.wait_for_scan(Duration::from_secs(120));
    shared_picker
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
        ..Default::default()
    }
}

fn fuzzy_options() -> GrepSearchOptions {
    GrepSearchOptions {
        mode: GrepMode::Fuzzy,
        ..plain_options()
    }
}

const PLAIN_QUERIES: &[(&str, &str)] = &[
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

const FUZZY_QUERIES: &[(&str, &str)] = &[
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

fn bench_plain_warm(c: &mut Criterion) {
    let data = setup();
    let opts = plain_options();

    let mut group = c.benchmark_group("plain_warm");
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(5));

    for (name, query) in PLAIN_QUERIES {
        group.bench_with_input(BenchmarkId::from_parameter(name), query, |b, q| {
            let guard = data.shared_picker.read().expect("read lock");
            let picker = guard.as_ref().expect("picker present");
            b.iter(|| {
                let parsed = parse_grep_query(q);
                black_box(picker.grep(&parsed, &opts))
            });
        });
    }

    group.finish();
}

fn bench_fuzzy_warm(c: &mut Criterion) {
    let data = setup();
    let opts = fuzzy_options();

    let mut group = c.benchmark_group("fuzzy_warm");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(8));

    for (name, query) in FUZZY_QUERIES {
        group.bench_with_input(BenchmarkId::from_parameter(name), query, |b, q| {
            let guard = data.shared_picker.read().expect("read lock");
            let picker = guard.as_ref().expect("picker present");
            b.iter(|| {
                let parsed = parse_grep_query(q);
                black_box(picker.grep(&parsed, &opts))
            });
        });
    }

    group.finish();
}

fn bench_plain_cold(c: &mut Criterion) {
    let _ = setup();
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

    for (name, query) in queries {
        group.bench_with_input(BenchmarkId::from_parameter(name), query, |b, q| {
            b.iter_with_setup(
                || setup_cold(),
                |cold_picker| {
                    let guard = cold_picker.read().expect("read lock");
                    let picker = guard.as_ref().expect("picker present");
                    let parsed = parse_grep_query(q);
                    let result = picker.grep(&parsed, &opts);
                    black_box(result.matches.len())
                },
            );
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_plain_warm,
    bench_fuzzy_warm,
    bench_plain_cold,
);

criterion_main!(benches);
