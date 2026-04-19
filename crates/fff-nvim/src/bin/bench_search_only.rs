/// Simple search profiler that directly uses scan_filesystem without background thread overhead
use fff::file_picker::FilePicker;
use fff::{FuzzySearchOptions, PaginationArgs, QueryParser};
use std::time::Instant;

fn load_picker(path: &std::path::Path) -> FilePicker {
    let mut picker = FilePicker::new(fff::FilePickerOptions {
        base_path: path.to_string_lossy().to_string(),
        enable_mmap_cache: false,
        mode: fff::FFFMode::Neovim,
        ..Default::default()
    })
    .expect("Failed to create FilePicker");
    picker.collect_files().expect("Failed to collect files");
    picker
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

    eprintln!("Loading files from: {:?}", canonical_path);

    let start = Instant::now();
    let picker = load_picker(&canonical_path);
    eprintln!(
        "✓ Loaded {} files in {:.2}s\n",
        picker.get_files().len(),
        start.elapsed().as_secs_f64()
    );

    // Test queries
    let test_queries = vec![
        ("short_common", "mod", 500),
        ("medium_specific", "controller", 200),
        ("long_rare", "user_authentication", 100),
        ("typo_resistant", "contrlr", 200),
        ("path_like", "src/lib", 150),
        ("two_char", "st", 300),
        ("partial_word", "test", 200),
        ("deep_path", "drivers/net", 100),
        ("extension", ".rs", 200),
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
            let parser = QueryParser::default();
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
    eprintln!(
        "\nYou can now run: perf record -g --call-graph dwarf -F 999 ./target/release/search_only"
    );
}
