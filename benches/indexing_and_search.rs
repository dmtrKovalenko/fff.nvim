use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use fff_nvim::FILE_PICKER;
use fff_nvim::file_picker::FilePicker;
use std::path::PathBuf;
use std::time::Duration;
use tracing_subscriber;

/// Initialize tracing to output to console
fn init_tracing() {
    // use tracing_subscriber::EnvFilter;
    // use tracing_subscriber::fmt;
    // let _ = fmt()
    //     .with_env_filter(
    //         EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
    //     )
    //     .with_target(false)
    //     .with_thread_ids(true)
    //     .with_line_number(true)
    //     .try_init();
}

/// Initialize FilePicker and insert into global state
fn init_file_picker_internal(path: &str) -> Result<(), String> {
    let picker = FilePicker::new(path.to_string())
        .map_err(|e| format!("Failed to create FilePicker: {:?}", e))?;

    let mut picker_guard = FILE_PICKER
        .write()
        .map_err(|_| "Failed to acquire write lock")?;
    *picker_guard = Some(picker);
    Ok(())
}

/// Helper function to wait for scanning to complete and get file count
fn wait_for_scan_completion(timeout_secs: u64) -> Result<usize, String> {
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let mut last_log = std::time::Instant::now();
    let mut iteration = 0;

    loop {
        iteration += 1;

        {
            let picker_guard = FILE_PICKER
                .read()
                .map_err(|_| "Failed to acquire read lock")?;
            if let Some(ref picker) = *picker_guard {
                let is_scanning = picker.is_scan_active();
                let file_count = picker.get_files().len();

                // Log progress every 2 seconds
                if last_log.elapsed() >= Duration::from_secs(2) {
                    eprintln!(
                        "  [{:.1}s] Scanning: {}, Files: {}, Iterations: {}",
                        start.elapsed().as_secs_f32(),
                        is_scanning,
                        file_count,
                        iteration
                    );
                    last_log = std::time::Instant::now();
                }

                if !is_scanning && file_count > 0 {
                    eprintln!(
                        "  ✓ Scan complete after {:.2}s: {} files found",
                        start.elapsed().as_secs_f32(),
                        file_count
                    );
                    return Ok(file_count);
                }
            } else {
                if iteration % 100 == 0 {
                    eprintln!(
                        "  [{:.1}s] FilePicker is None (iteration {})",
                        start.elapsed().as_secs_f32(),
                        iteration
                    );
                }
            }
        }

        if start.elapsed() > timeout {
            return Err(format!(
                "Scan timed out after {} seconds (iteration {})",
                timeout_secs, iteration
            ));
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Get files from the global FILE_PICKER
fn get_files_snapshot() -> Result<Vec<fff_nvim::types::FileItem>, String> {
    let picker_guard = FILE_PICKER
        .read()
        .map_err(|_| "Failed to acquire read lock")?;
    if let Some(ref picker) = *picker_guard {
        Ok(picker.get_files().to_vec())
    } else {
        Err("FilePicker not initialized".to_string())
    }
}

/// Clean up global state
fn cleanup_global_state() {
    if let Ok(mut picker_guard) = FILE_PICKER.write() {
        if let Some(mut picker) = picker_guard.take() {
            picker.stop_background_monitor();
        }
    }
}

/// Initialize FilePicker once and return files snapshot
fn setup_once() -> Result<Vec<fff_nvim::types::FileItem>, String> {
    init_tracing();

    let big_repo_path = PathBuf::from("./big-repo");
    if !big_repo_path.exists() {
        return Err("./big-repo directory does not exist. Run git clone https://github.com/torvalds/linux.git big-repo".to_string());
    }

    let canonical_path = big_repo_path
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize path: {}", e))?;
    eprintln!("  Path: {:?}", canonical_path);

    {
        let picker_guard = FILE_PICKER
            .read()
            .map_err(|_| "Failed to acquire read lock")?;
        if let Some(ref picker) = *picker_guard {
            let files = picker.get_files();
            if !files.is_empty() {
                eprintln!("  ℹ Reusing existing index with {} files", files.len());
                return Ok(files.to_vec());
            }
        }
    }

    cleanup_global_state();
    std::thread::sleep(Duration::from_millis(500));

    init_file_picker_internal(&canonical_path.to_string_lossy())?;

    eprintln!("  Waiting for background scan to complete...");
    let file_count = wait_for_scan_completion(120)?;
    eprintln!(
        "  ✓ Indexed {} files (will be reused for all benchmarks)\n",
        file_count
    );

    get_files_snapshot()
}

/// Benchmark for indexing the big-repo directory
fn bench_indexing(c: &mut Criterion) {
    init_tracing();

    let big_repo_path = PathBuf::from("./big-repo");
    if !big_repo_path.exists() {
        eprintln!(
            "./big-repo directory does not exist. Run git clone https://github.com/torvalds/linux.git big-repo"
        );
        return;
    }

    let canonical_path = match big_repo_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("⚠ Failed to canonicalize path: {}", e);
            return;
        }
    };

    let mut group = c.benchmark_group("indexing");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(20));

    group.bench_function("index_big_repo", |b| {
        b.iter(|| {
            cleanup_global_state();
            std::thread::sleep(Duration::from_millis(500));

            let start = std::time::Instant::now();
            init_file_picker_internal(black_box(&canonical_path.to_string_lossy()))
                .expect("Failed to init FilePicker");

            match wait_for_scan_completion(120) {
                Ok(file_count) => {
                    let elapsed = start.elapsed();
                    eprintln!("  ✓ Indexed {} files in {:?}", file_count, elapsed);
                    file_count
                }
                Err(e) => {
                    eprintln!("  ✗ Error: {}", e);
                    0
                }
            }
        });
    });

    group.finish();
}

/// Benchmark for searching with various query patterns
fn bench_search_queries(c: &mut Criterion) {
    let files = match setup_once() {
        Ok(files) => files,
        Err(e) => {
            eprint!("Failed to setup picker {e:?}");
            return;
        }
    };

    let mut group = c.benchmark_group("search");
    group.sample_size(100);

    let test_queries = vec![
        ("short", "mod"),
        ("medium", "controller"),
        ("long", "user_authentication"),
        ("typo", "contrlr"),
        ("partial", "src/lib"),
    ];

    for (name, query) in test_queries {
        group.bench_with_input(BenchmarkId::new("query", name), &query, |b, &query| {
            b.iter(|| {
                let results = FilePicker::fuzzy_search(
                    black_box(&files),
                    black_box(query),
                    black_box(100),
                    black_box(4),
                    black_box(None),
                    black_box(false),
                );
                results.total_matched
            });
        });
    }

    group.finish();
}

/// Benchmark search with different thread counts
fn bench_search_thread_scaling(c: &mut Criterion) {
    let files = match setup_once() {
        Ok(files) => files,
        Err(e) => {
            eprintln!("⚠ Skipping thread scaling benchmarks: {}", e);
            return;
        }
    };

    let mut group = c.benchmark_group("thread_scaling");
    group.sample_size(100);

    let query = "controller";
    let thread_counts = vec![1, 2, 4, 8];

    for threads in thread_counts {
        group.bench_with_input(
            BenchmarkId::from_parameter(threads),
            &threads,
            |b, &threads| {
                b.iter(|| {
                    let results = FilePicker::fuzzy_search(
                        black_box(&files),
                        black_box(query),
                        black_box(100),
                        black_box(threads),
                        black_box(None),
                        black_box(false),
                    );
                    results.total_matched
                });
            },
        );
    }

    group.finish();
}

/// Benchmark search with different result limits
fn bench_search_result_limits(c: &mut Criterion) {
    let files = match setup_once() {
        Ok(files) => files,
        Err(e) => {
            eprintln!("⚠ Skipping result limit benchmarks: {}", e);
            return;
        }
    };

    let mut group = c.benchmark_group("result_limits");
    group.sample_size(100);

    let query = "mod";
    let result_limits = vec![10, 50, 100, 500];

    for limit in result_limits {
        group.bench_with_input(BenchmarkId::from_parameter(limit), &limit, |b, &limit| {
            b.iter(|| {
                let results = FilePicker::fuzzy_search(
                    black_box(&files),
                    black_box(query),
                    black_box(limit),
                    black_box(4),
                    black_box(None),
                    black_box(false),
                );
                results.total_matched
            });
        });
    }

    group.finish();
}

/// Benchmark search algorithm performance scaling with file count
fn bench_search_scalability(c: &mut Criterion) {
    let all_files = match setup_once() {
        Ok(files) => files,
        Err(e) => {
            eprintln!("⚠ Skipping scalability benchmarks: {}", e);
            return;
        }
    };

    if all_files.len() < 1000 {
        eprintln!(
            "⚠ Skipping scalability benchmark: need at least 1000 files, got {}",
            all_files.len()
        );
        return;
    }

    let mut group = c.benchmark_group("search_scalability");
    group.sample_size(50);

    let query = "controller";
    let file_counts = vec![100, 1000, 5000, 10000, all_files.len().min(50000)];

    for count in file_counts {
        if count > all_files.len() {
            continue;
        }

        let subset = &all_files[..count];
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, _| {
            b.iter(|| {
                let results = FilePicker::fuzzy_search(
                    black_box(subset),
                    black_box(query),
                    black_box(100),
                    black_box(4),
                    black_box(None),
                    black_box(false),
                );
                results.total_matched
            });
        });
    }

    group.finish();
}

/// Benchmark search performance with different ordering modes
fn bench_search_ordering(c: &mut Criterion) {
    let files = match setup_once() {
        Ok(files) => files,
        Err(e) => {
            eprintln!("⚠ Skipping ordering benchmarks: {}", e);
            return;
        }
    };

    let mut group = c.benchmark_group("ordering");
    group.sample_size(100);

    let query = "controller";

    // Benchmark normal order (descending)
    group.bench_function("normal_order", |b| {
        b.iter(|| {
            let results = FilePicker::fuzzy_search(
                black_box(&files),
                black_box(query),
                black_box(100),
                black_box(4),
                black_box(None),
                black_box(false),
            );
            results.total_matched
        });
    });

    // Benchmark reverse order (ascending)
    group.bench_function("reverse_order", |b| {
        b.iter(|| {
            let results = FilePicker::fuzzy_search(
                black_box(&files),
                black_box(query),
                black_box(100),
                black_box(4),
                black_box(None),
                black_box(true),
            );
            results.total_matched
        });
    });

    // Benchmark with large result set
    group.bench_function("normal_order_large", |b| {
        b.iter(|| {
            let results = FilePicker::fuzzy_search(
                black_box(&files),
                black_box("mod"),
                black_box(500),
                black_box(4),
                black_box(None),
                black_box(false),
            );
            results.total_matched
        });
    });

    group.bench_function("reverse_order_large", |b| {
        b.iter(|| {
            let results = FilePicker::fuzzy_search(
                black_box(&files),
                black_box("mod"),
                black_box(500),
                black_box(4),
                black_box(None),
                black_box(true),
            );
            results.total_matched
        });
    });

    // Benchmark with small result set
    group.bench_function("normal_order_small", |b| {
        b.iter(|| {
            let results = FilePicker::fuzzy_search(
                black_box(&files),
                black_box("controller"),
                black_box(10),
                black_box(4),
                black_box(None),
                black_box(false),
            );
            results.total_matched
        });
    });

    group.bench_function("reverse_order_small", |b| {
        b.iter(|| {
            let results = FilePicker::fuzzy_search(
                black_box(&files),
                black_box("controller"),
                black_box(10),
                black_box(4),
                black_box(None),
                black_box(true),
            );
            results.total_matched
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_indexing,
    bench_search_queries,
    bench_search_thread_scaling,
    bench_search_result_limits,
    bench_search_scalability,
    bench_search_ordering,
);

criterion_main!(benches);
