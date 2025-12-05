use fff_nvim::FILE_PICKER;
use fff_nvim::file_picker::FilePicker;
use std::time::{Duration, Instant};

/// Wait for background scan to complete
fn wait_for_scan(timeout_secs: u64) -> Result<usize, String> {
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let mut iteration = 0;

    loop {
        iteration += 1;

        let picker_guard = FILE_PICKER
            .read()
            .map_err(|_| "Failed to acquire read lock")?;
        if let Some(ref picker) = *picker_guard {
            let is_scanning = picker.is_scan_active();
            let file_count = picker.get_files().len();

            if iteration % 20 == 0 {
                eprintln!(
                    "  [{:.1}s] Scanning: {}, Files: {}",
                    start.elapsed().as_secs_f64(),
                    is_scanning,
                    file_count
                );
            }

            if !is_scanning && file_count > 0 {
                return Ok(file_count);
            }
        } else if iteration % 20 == 0 {
            eprintln!(
                "  [{:.1}s] FilePicker is None",
                start.elapsed().as_secs_f64()
            );
        }

        if start.elapsed() > timeout {
            return Err(format!("Scan timed out after {} seconds", timeout_secs));
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Initialize FilePicker and insert into global state
fn init_file_picker(path: &str) -> Result<(), String> {
    let picker = FilePicker::new(path.to_string())
        .map_err(|e| format!("Failed to create FilePicker: {:?}", e))?;

    let mut picker_guard = FILE_PICKER
        .write()
        .map_err(|_| "Failed to acquire write lock")?;
    *picker_guard = Some(picker);
    Ok(())
}

/// Get files snapshot from global state
fn get_files() -> Result<Vec<fff_nvim::types::FileItem>, String> {
    let picker_guard = FILE_PICKER
        .read()
        .map_err(|_| "Failed to acquire read lock")?;
    if let Some(ref picker) = *picker_guard {
        Ok(picker.get_files().to_vec())
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

    let canonical_path = big_repo_path
        .canonicalize()
        .expect("Failed to canonicalize path");

    eprintln!("Initializing FilePicker for: {:?}", canonical_path);
    init_file_picker(&canonical_path.to_string_lossy()).expect("Failed to init FilePicker");

    // Give background thread time to start
    std::thread::sleep(Duration::from_millis(200));

    eprintln!("Waiting for scan to complete...");
    let file_count = wait_for_scan(120).expect("Failed to wait for scan");
    eprintln!("✓ Indexed {} files\n", file_count);

    let files = get_files().expect("Failed to get files");

    // Test queries representing different search patterns
    let test_queries = vec![
        ("short_common", "mod", 5000),
        ("medium_specific", "controller", 2000),
        ("long_rare", "user_authentication", 1000),
        ("typo_resistant", "contrlr", 2000),
        ("path_like", "src/lib", 1500),
        ("single_char", "a", 3000),
        ("two_char", "st", 3000),
        ("partial_word", "test", 2000),
        ("deep_path", "drivers/net", 1000),
        ("extension", ".rs", 2000),
    ];

    eprintln!("Running search profiler...");
    eprintln!("Query                 | Iterations | Total Time | Avg Time  | Matches");
    eprintln!("----------------------|------------|------------|-----------|--------");

    let global_start = Instant::now();
    let mut total_iterations = 0;

    for (name, query, iterations) in test_queries {
        let start = Instant::now();
        let mut match_count = 0;

        for _ in 0..iterations {
            let results = FilePicker::fuzzy_search(
                &files, query, 100,   // max_results
                4,     // max_threads
                None,  // current_file
                false, // reverse_order
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
