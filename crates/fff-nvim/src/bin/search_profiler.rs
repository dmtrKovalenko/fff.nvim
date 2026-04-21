use fff::file_picker::{FFFMode, FilePicker};
use fff::{FuzzySearchOptions, PaginationArgs, QueryParser, SharedFrecency, SharedPicker};
use std::time::{Duration, Instant};

/// Wait for background scan to complete
fn wait_for_scan(shared_picker: &SharedPicker, timeout_secs: u64) -> Result<usize, String> {
    let timeout = Duration::from_secs(timeout_secs);
    if !shared_picker.wait_for_scan(timeout) {
        return Err(format!("Scan timed out after {} seconds", timeout_secs));
    }

    let picker_guard = shared_picker
        .read()
        .map_err(|e| format!("Failed to acquire read lock: {}", e))?;
    if let Some(ref picker) = *picker_guard {
        Ok(picker.get_files().len())
    } else {
        Err("FilePicker not initialized".to_string())
    }
}

fn main() {
    let big_repo_path = std::path::PathBuf::from("./big-repo");

    if !big_repo_path.exists() {
        eprintln!(
            "./big-repo directory does not exist. Run git clone https://github.com/torvalds/linux.git big-repo"
        );
        return;
    }

    let canonical_path =
        fff::path_utils::canonicalize(&big_repo_path).expect("Failed to canonicalize path");

    // Create shared state
    let shared_picker = SharedPicker::default();
    let shared_frecency = SharedFrecency::default();

    eprintln!("Initializing FilePicker for: {:?}", canonical_path);
    FilePicker::new_with_shared_state(
        shared_picker.clone(),
        shared_frecency.clone(),
        fff::FilePickerOptions {
            base_path: canonical_path.to_string_lossy().to_string(),
            enable_mmap_cache: false,
            mode: FFFMode::Neovim,
            ..Default::default()
        },
    )
    .expect("Failed to init FilePicker with shared state");

    // Give background thread time to start
    std::thread::sleep(Duration::from_millis(200));

    eprintln!("Waiting for scan to complete...");
    let file_count = wait_for_scan(&shared_picker, 120).expect("Failed to wait for scan");
    eprintln!("✓ Indexed {} files\n", file_count);

    let picker_guard = shared_picker.read().expect("Failed to acquire read lock");
    let picker = picker_guard.as_ref().expect("FilePicker not initialized");

    // Test queries representing different search patterns
    let test_queries = vec![
        ("short_common", "mod", 100),
        ("medium_specific", "controller", 100),
        ("long_rare", "user_authentication", 100),
        ("typo_resistant", "contrlr", 100),
        ("path_like", "src/lib", 100),
        ("single_char", "a", 100),
        ("two_char", "st", 100),
        ("partial_word", "test", 100),
        ("deep_path", "drivers/net", 100),
        ("extension", ".rs", 100),
    ];

    eprintln!("Running search profiler...");
    eprintln!("Query                 | Iterations | Total Time | Avg Time  | Matches");
    eprintln!("----------------------|------------|------------|-----------|--------");

    let global_start = Instant::now();
    let mut total_iterations = 0;

    for (name, query, iterations) in test_queries {
        let start = Instant::now();
        let mut match_count = 0;
        let parser = QueryParser::default();

        for _ in 0..iterations {
            let parsed = parser.parse(query);
            let results = picker.fuzzy_search(
                &parsed,
                None,
                FuzzySearchOptions {
                    max_threads: 4,
                    current_file: None,
                    project_path: None,
                    combo_boost_score_multiplier: 100,
                    min_combo_count: 3,
                    pagination: PaginationArgs {
                        offset: 0,
                        limit: 100,
                    },
                },
            );

            match_count += results.total_matched;
        }

        let elapsed = start.elapsed();
        let avg_time = elapsed / iterations as u32;

        eprintln!(
            "{:<21} | {:>10} | {:>9.2}s | {:>7}µs | {}",
            name,
            iterations,
            elapsed.as_secs_f64(),
            avg_time.as_micros(),
            match_count / iterations
        );

        total_iterations += iterations;
    }

    let total_time = global_start.elapsed();

    eprintln!("\n=== Summary ===");
    eprintln!("Total searches:     {}", total_iterations);
    eprintln!("Total time:         {:.2}s", total_time.as_secs_f64());
    eprintln!(
        "Average per search: {}µs",
        (total_time.as_micros() as usize) / total_iterations
    );
    eprintln!(
        "Searches per sec:   {:.0}",
        total_iterations as f64 / total_time.as_secs_f64()
    );

    // Keep the program alive briefly so perf can capture everything
    std::thread::sleep(Duration::from_millis(100));
}
