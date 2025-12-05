/// Simple search profiler that directly uses scan_filesystem without background thread overhead
use fff_nvim::file_picker::FilePicker;
use std::time::Instant;

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

    eprintln!("Loading files from: {:?}", canonical_path);

    // Directly scan without background thread
    let start = Instant::now();
    let files = {
        use ignore::WalkBuilder;
        let mut files = Vec::new();

        WalkBuilder::new(&canonical_path)
            .hidden(false)
            .build()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_some_and(|ft| ft.is_file()))
            .for_each(|entry| {
                let path = entry.path().to_path_buf();
                let relative =
                    pathdiff::diff_paths(&path, &canonical_path).unwrap_or_else(|| path.clone());

                let relative_path = relative.to_string_lossy().into_owned();
                let file_name = entry.file_name().to_string_lossy().into_owned();

                files.push(fff_nvim::types::FileItem {
                    path,
                    relative_path_lower: relative_path.to_lowercase(),
                    relative_path,
                    file_name_lower: file_name.to_lowercase(),
                    file_name,
                    size: entry.metadata().ok().map_or(0, |m| m.len()),
                    modified: 0,
                    access_frecency_score: 0,
                    modification_frecency_score: 0,
                    total_frecency_score: 0,
                    git_status: None,
                });
            });

        files
    };

    eprintln!(
        "✓ Loaded {} files in {:.2}s\n",
        files.len(),
        start.elapsed().as_secs_f64()
    );

    // Test queries
    let test_queries = vec![
        ("short_common", "mod", 500),
        ("medium_specific", "controller", 200),
        ("long_rare", "user_authentication", 100),
        ("typo_resistant", "contrlr", 200),
        ("path_like", "src/lib", 150),
        ("single_char", "a", 300),
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
    eprintln!(
        "\nYou can now run: perf record -g --call-graph dwarf -F 999 ./target/release/search_only"
    );
}
