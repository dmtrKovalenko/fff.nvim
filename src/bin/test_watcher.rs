#[path = "../../lua/fff/rust/error.rs"]
mod error;
#[path = "../../lua/fff/rust/file_key.rs"]
mod file_key;
#[path = "../../lua/fff/rust/file_picker.rs"]
mod file_picker;
#[path = "../../lua/fff/rust/frecency.rs"]
mod frecency;
#[path = "../../lua/fff/rust/git.rs"]
mod git;
#[path = "../../lua/fff/rust/path_utils.rs"]
mod path_utils;
#[path = "../../lua/fff/rust/score.rs"]
mod score;
#[path = "../../lua/fff/rust/types.rs"]
mod types;

use file_picker::FilePicker;
use frecency::FrecencyTracker;
use std::env;
use std::io::{self, Write};
use std::sync::{LazyLock, RwLock};
use std::thread;
use std::time::Duration;

use crate::git::format_git_status;

static FRECENCY: LazyLock<RwLock<Option<FrecencyTracker>>> = LazyLock::new(|| RwLock::new(None));

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let base_path = if args.len() > 1 {
        args[1].clone()
    } else {
        env::current_dir()?.to_str().unwrap_or(".").to_string()
    };

    let picker = match FilePicker::new(base_path.clone()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("❌ Failed to create FilePicker: {:?}", e);
            std::process::exit(1);
        }
    };

    let initial_files = picker.get_cached_files();
    println!("Initial file count: {}", initial_files.len());

    if !initial_files.is_empty() {
        println!("Sample files:");
        for (i, file) in initial_files.iter().take(5).enumerate() {
            println!(
                "  {}. {} ({})",
                i + 1,
                file.relative_path,
                format_git_status(file.git_status)
            );
        }
        if initial_files.len() > 5 {
            println!("  ... and {} more files", initial_files.len() - 5);
        }
    }

    println!("{:=<60}", "");
    println!("🔴 LIVE FILE MONITORING - Press Ctrl+C to stop");
    println!("{:=<60}", "");

    let mut last_count = initial_files.len();
    let mut iteration = 0;

    loop {
        thread::sleep(Duration::from_millis(500));
        iteration += 1;

        let current_files = picker.get_cached_files();
        let current_count = current_files.len();

        if current_count != last_count {
            let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");

            if current_count > last_count {
                let added = current_count - last_count;
                println!(
                    "🟢 [{}] +{} files added | Total: {}",
                    timestamp, added, current_count
                );

                if let Some(newest_files) = current_files
                    .iter()
                    .rev()
                    .take(added)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .get(0..added.min(3))
                {
                    for file in newest_files {
                        println!("   ➕ {}", file.relative_path);
                    }
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

            let mut git_stats = std::collections::HashMap::new();
            for file in &current_files {
                let status = format_git_status(file.git_status);
                *git_stats.entry(status).or_insert(0) += 1;
            }

            let git_stats_copy = git_stats.clone();

            if !git_stats.is_empty() {
                print!("   📊 Git status: ");
                for (status, count) in &git_stats {
                    print!("{}:{} ", status, count);
                }
                println!();
            }

            println!("   🔄 Testing git status refresh...");
            let refreshed_files = picker.refresh_git_status();
            let mut new_git_stats = std::collections::HashMap::new();
            for file in &refreshed_files {
                let status = format_git_status(file.git_status);
                *new_git_stats.entry(status).or_insert(0) += 1;
            }
            if new_git_stats != git_stats_copy {
                print!("   ✨ Git status changed after refresh: ");
                for (status, count) in &new_git_stats {
                    print!("{}:{} ", status, count);
                }
                println!();
            }
        }

        if iteration % 40 == 0 {
            let search_results = picker.fuzzy_search("rs", 5, 2, None);
            let timestamp = chrono::Local::now().format("%H:%M:%S");
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
}
