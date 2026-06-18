//! Benchmarks capturing performance of index building and fs walking
use criterion::{Criterion, criterion_group, criterion_main};
use fff::file_picker::{FFFMode, FilePicker};
use fff::{FilePickerOptions, SharedFilePicker, SharedFrecency};
use std::path::PathBuf;
use std::sync::Once;
use std::time::{Duration, Instant};

const WAIT_TIMEOUT: Duration = Duration::from_secs(300);

static TRACING_INIT: Once = Once::new();

fn init_tracing() {
    TRACING_INIT.call_once(|| {
        use tracing_subscriber::EnvFilter;
        use tracing_subscriber::fmt::format::FmtSpan;

        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new("warn,fff_search=info")),
            )
            .with_span_events(FmtSpan::CLOSE)
            .with_target(true)
            .with_writer(std::io::stderr)
            .try_init();
    });
}

fn resolve_repo() -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("FFF_BENCH_REPO") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return fff::path_utils::canonicalize(&p).ok();
        }
    }
    // Resolve relative to the workspace root (two levels up from this crate).
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let default = workspace_root.join("big-repo");
    if default.exists() {
        return fff::path_utils::canonicalize(&default).ok();
    }
    None
}

fn init_picker(
    path: &str,
    content_indexing: bool,
    warmup: bool,
) -> (SharedFilePicker, SharedFrecency) {
    let sp = SharedFilePicker::default();
    let sf = SharedFrecency::default();
    FilePicker::new_with_shared_state(
        sp.clone(),
        sf.clone(),
        FilePickerOptions {
            base_path: path.to_string(),
            enable_mmap_cache: warmup,
            enable_content_indexing: content_indexing,
            mode: FFFMode::Neovim,
            watch: false,
            ..Default::default()
        },
    )
    .expect("init FilePicker");
    (sp, sf)
}

/// Portable wait across branches (no dependency on `wait_for_indexing_complete`).
/// Polls until the scan is inactive AND the bigram index has been installed — this
/// is the end of `run_post_scan` for content-indexing builds.
fn wait_for_post_scan(sp: &SharedFilePicker, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let ready = sp
            .read()
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .map(|p| !p.is_scan_active() && !p.is_post_scan_active())
            })
            .unwrap_or(false);
        if ready {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Portable wait for walk-only builds (content_indexing=false, no bigram).
fn wait_for_scan_done(sp: &SharedFilePicker, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let done = sp
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(|p| !p.is_scan_active()))
            .unwrap_or(false);
        if done {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn cleanup(sp: SharedFilePicker) {
    // Clean teardown: wait for scan + post-scan to finish, then drop.
    // On the refactored branch `wait_for_indexing_complete` guarantees
    // no outstanding snapshots remain so the picker can be torn down
    // safely. On the pre-refactor baseline this would UAF because the
    // picker can drop while post-scan threads still hold raw pointers
    // into its storage.
    sp.wait_for_indexing_complete(WAIT_TIMEOUT);
    if let Ok(mut guard) = sp.write()
        && let Some(mut picker) = guard.take()
    {
        picker.stop_background_monitor();
    }
}

fn bench_full_init(c: &mut Criterion) {
    init_tracing();
    let Some(repo) = resolve_repo() else {
        eprintln!("skip: set FFF_BENCH_REPO or clone a repo to ./big-repo");
        return;
    };
    let path = repo.to_string_lossy().to_string();
    eprintln!("post_scan_bench repo: {}", path);

    let mut group = c.benchmark_group("post_scan");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(120));

    group.bench_function("full_init", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                let (sp, _sf) = init_picker(&path, true, true);
                wait_for_post_scan(&sp, WAIT_TIMEOUT);
                total += start.elapsed();
                cleanup(sp);
            }
            total
        });
    });

    group.finish();
}

fn bench_post_scan_only(c: &mut Criterion) {
    init_tracing();
    let Some(repo) = resolve_repo() else {
        return;
    };
    let path = repo.to_string_lossy().to_string();

    let mut group = c.benchmark_group("post_scan");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(120));

    group.bench_function("post_scan_only", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let (sp, _sf) = init_picker(&path, true, true);
                wait_for_scan_done(&sp, WAIT_TIMEOUT);
                let start = Instant::now();
                wait_for_post_scan(&sp, WAIT_TIMEOUT);
                total += start.elapsed();
                cleanup(sp);
            }
            total
        });
    });

    group.finish();
}

fn bench_walk_only(c: &mut Criterion) {
    init_tracing();
    let Some(repo) = resolve_repo() else {
        return;
    };
    let path = repo.to_string_lossy().to_string();

    let mut group = c.benchmark_group("post_scan");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(60));

    group.bench_function("walk_only", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                let (sp, _sf) = init_picker(&path, false, false);
                wait_for_scan_done(&sp, WAIT_TIMEOUT);
                total += start.elapsed();
                cleanup(sp);
            }
            total
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_full_init,
    bench_post_scan_only,
    bench_walk_only
);
criterion_main!(benches);
