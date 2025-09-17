#![allow(clippy::all)]
#![allow(dead_code)]
#![allow(clippy::enum_variant_names)]

use fff_nvim::{file_picker::FilePicker, git::format_git_status, FILE_PICKER, FRECENCY};
use std::env;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn cleanup_global_state() {
    // Clean up file picker
    {
        let mut file_picker = FILE_PICKER.write().unwrap();
        if let Some(mut picker) = file_picker.take() {
            let _ = picker.stop_background_monitor();
            drop(picker);
            println!("🧹 FilePicker cleaned up");
        }
    }

    // Clean up frecency tracker
    {
        let mut frecency = FRECENCY.write().unwrap();
        *frecency = None;
        println!("🧹 Frecency tracker cleaned up");
    }
}

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

    ctrlc::set_handler(move || {
        println!("\n🛑 Received interrupt signal, shutting down...");
        cleanup_global_state();
        r.store(false, Ordering::SeqCst);
        std::process::exit(0);
    })?;

    let mut git_stats = std::collections::HashMap::new();
    // Initialize the global file picker using lib.rs function
    {
        let mut file_picker = FILE_PICKER.write().unwrap();
        if file_picker.is_some() {
            eprintln!("❌ FilePicker already initialized");
            std::process::exit(1);
        }
        *file_picker = Some(FilePicker::new(base_path.clone())?);
    }

    // Get initial file count from global state
    let initial_count = {
        let file_picker = FILE_PICKER.read().unwrap();
        let files = file_picker.as_ref().unwrap().get_files();
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
            let file_picker = FILE_PICKER.read().unwrap();
            file_picker.as_ref().unwrap().get_files().len()
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
                let file_picker = FILE_PICKER.read().unwrap();
                let files = file_picker.as_ref().unwrap().get_files();
                let newest_files = files.iter().rev().take(added.min(3));
                for file in newest_files {
                    println!("   ➕ {}", file.relative_path);
                }
                drop(file_picker);
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

            let file_picker = FILE_PICKER.read().unwrap();
            let current_files = file_picker.as_ref().unwrap().get_files();

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
            let file_picker = FILE_PICKER.read().unwrap();
            let files = file_picker.as_ref().unwrap().get_files();
            let search_results = FilePicker::fuzzy_search(files, "rs", 5, 2, None, false);

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
            drop(file_picker);
        }

        io::stdout().flush().unwrap();
    }

    // Clean up before exit
    cleanup_global_state();
    Ok(())
}
