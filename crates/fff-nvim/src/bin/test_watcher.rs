#![allow(clippy::all)]
#![allow(dead_code)]
#![allow(clippy::enum_variant_names)]

use fff_core::file_picker::FilePicker;
use fff_core::git::format_git_status;
use fff_core::{FuzzySearchOptions, PaginationArgs, QueryParser, SharedFrecency, SharedPicker};
use std::env;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let base_path = if args.len() > 1 {
        args[1].clone()
    } else {
        env::current_dir()?.to_str().unwrap_or(".").to_string()
    };

    // Set up signal handler for graceful shutdown
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    // Create shared state
    let shared_picker: SharedPicker = Arc::new(RwLock::new(None));
    let shared_frecency: SharedFrecency = Arc::new(RwLock::new(None));

    // Clone for signal handler
    let picker_for_cleanup = Arc::clone(&shared_picker);
    ctrlc::set_handler(move || {
        println!("\n🛑 Received interrupt signal, shutting down...");
        if let Ok(mut guard) = picker_for_cleanup.write() {
            if let Some(mut picker) = guard.take() {
                picker.stop_background_monitor();
                println!("🧹 FilePicker cleaned up");
            }
        }
        r.store(false, Ordering::SeqCst);
        std::process::exit(0);
    })?;

    let mut git_stats = std::collections::HashMap::new();

    // Initialize the file picker using shared state
    FilePicker::new_with_shared_state(
        base_path.clone(),
        false,
        Arc::clone(&shared_picker),
        Arc::clone(&shared_frecency),
    )?;

    // Get initial file count from shared state
    let initial_count = {
        let guard = shared_picker.read().unwrap();
        let files = guard.as_ref().unwrap().get_files();
        println!("Initial file count: {}", files.len());

        if !files.is_empty() {
            println!("Sample files:");
            for (i, file) in files.iter().take(5).enumerate() {
                println!(
                    "  {}. {} ({})",
                    i + 1,
                    file.relative_path,
                    format_git_status(file.git_status)
                );
            }
            if files.len() > 5 {
                println!("  ... and {} more files", files.len() - 5);
            }
        }
        files.len()
    };

    println!("{:=<60}", "");
    println!("🔴 LIVE FILE MONITORING - Press Ctrl+C to stop");
    println!("{:=<60}", "");

    let mut last_count = initial_count;
    let mut iteration = 0;

    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(500));
        iteration += 1;

        let current_count = {
            let guard = shared_picker.read().unwrap();
            guard.as_ref().unwrap().get_files().len()
        };

        if current_count != last_count {
            let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");

            if current_count > last_count {
                let added = current_count - last_count;
                println!(
                    "🟢 [{}] +{} files added | Total: {}",
                    timestamp, added, current_count
                );

                // Show some recently added files
                let guard = shared_picker.read().unwrap();
                let files = guard.as_ref().unwrap().get_files();
                let newest_files = files.iter().rev().take(added.min(3));
                for file in newest_files {
                    println!("   ➕ {}", file.relative_path);
                }
            } else {
                let removed = last_count - current_count;
                println!(
                    "🔴 [{}] -{} files removed | Total: {}",
                    timestamp, removed, current_count
                );
            }

            last_count = current_count;
        }

        if iteration % 20 == 0 {
            let timestamp = chrono::Local::now().format("%H:%M:%S");
            println!(
                "💓 [{}] Heartbeat - {} files cached, watcher active",
                timestamp, current_count
            );

            let guard = shared_picker.read().unwrap();
            let current_files = guard.as_ref().unwrap().get_files();

            git_stats.clear();
            for file in current_files {
                let status = format_git_status(file.git_status);
                *git_stats.entry(status).or_insert(0) += 1;
            }

            if !git_stats.is_empty() {
                print!("   📊 Git status: ");
                for (status, count) in &git_stats {
                    print!("{}:{} ", status, count);
                }
                println!();
            }
        }

        if iteration % 40 == 0 {
            let timestamp = chrono::Local::now().format("%H:%M:%S");
            let guard = shared_picker.read().unwrap();
            let files = guard.as_ref().unwrap().get_files();
            let parser = QueryParser::default();
            let parsed = parser.parse("rs");
            let search_results = FilePicker::fuzzy_search(
                files,
                "rs",
                parsed,
                FuzzySearchOptions {
                    max_threads: 2,
                    current_file: None,
                    project_path: None,
                    last_same_query_match: None,
                    combo_boost_score_multiplier: 100,
                    min_combo_count: 3,
                    pagination: PaginationArgs {
                        offset: 0,
                        limit: 5,
                    },
                },
            );

            println!(
                "🔍 [{}] Search test 'rs': {} matches",
                timestamp,
                search_results.items.len()
            );
            for (i, (file, score)) in search_results
                .items
                .iter()
                .zip(search_results.scores.iter())
                .take(3)
                .enumerate()
            {
                println!(
                    "   {}. {} (score: {})",
                    i + 1,
                    file.relative_path,
                    score.total
                );
            }
        }

        io::stdout().flush().unwrap();
    }

    // Clean up before exit
    if let Ok(mut guard) = shared_picker.write() {
        if let Some(mut picker) = guard.take() {
            picker.stop_background_monitor();
        }
    }
    Ok(())
}
