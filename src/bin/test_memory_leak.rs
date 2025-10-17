use fff_nvim::{FILE_PICKER, file_picker::FilePicker};
use std::env;
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

fn get_memory_usage() -> Result<u64, Box<dyn std::error::Error>> {
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        return Err("Memory usage check is only supported on Linux and macOS".into());
    }

    #[cfg(target_os = "macos")]
    {
        let pid = std::process::id();
        use std::process::Command;
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()?;

        let rss_str = String::from_utf8(output.stdout)?;
        let rss_kb: u64 = rss_str.trim().parse()?;

        Ok(rss_kb * 1024)
    }

    #[cfg(target_os = "linux")]
    {
        let pid = std::process::id();
        let status_path = format!("/proc/{}/status", pid);
        let content = std::fs::read_to_string(status_path)?;

        for line in content.lines() {
            if line.starts_with("VmRSS:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Ok(rss_kb) = parts[1].parse::<u64>()
                    && parts.len() >= 2
                {
                    return Ok(rss_kb * 1024); // Convert KB to bytes
                }
            }
        }

        Err("Could not find VmRSS in /proc/pid/status".into())
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_index])
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if !cfg!(target_os = "linux") && !cfg!(target_os = "macos") {
        eprintln!("This test is only supported on Linux and macOS.");
        std::process::exit(1);
    }

    let args: Vec<String> = env::args().collect();
    let base_path = if args.len() > 1 {
        args[1].clone()
    } else {
        env::current_dir()?.to_str().unwrap_or(".").to_string()
    };

    println!("🧪 FFF.nvim Memory Leak Test (Using Crate)");
    println!("==========================================");
    println!("Test directory: {}", base_path);
    println!();

    // Initialize the file picker directly
    println!("📁 Initializing FilePicker...");
    {
        let mut file_picker_guard = FILE_PICKER.write().unwrap();
        if file_picker_guard.is_none() {
            println!("Creating new FilePicker for path: {}", base_path);
            match FilePicker::new(base_path.clone()) {
                Ok(picker) => {
                    println!("FilePicker created successfully");
                    *file_picker_guard = Some(picker);
                }
                Err(e) => {
                    eprintln!("Failed to create FilePicker: {:?}", e);
                    std::process::exit(1);
                }
            }
        }
    }

    // Wait for initial scan to complete
    println!("⏳ Waiting for initial file scan to complete...");
    let mut wait_time = 0;
    let mut scan_completed = false;

    loop {
        if let Ok(file_picker_guard) = FILE_PICKER.read()
            && let Some(ref picker) = *file_picker_guard
        {
            if !picker.is_scan_active() {
                println!("Scan inactive, checking file count...");
                let file_count = picker.get_files().len();
                if file_count > 0 {
                    println!("Async scan found {} files", file_count);
                    scan_completed = true;
                    break;
                }
            } else {
                println!("Scan active, waiting...");
            }
        }
        thread::sleep(Duration::from_millis(100));
        wait_time += 100;
        if wait_time > 10000 {
            println!("Timeout waiting for async scan, triggering manual scan...");
            break;
        }
    }

    // If async scan didn't work, trigger a manual scan
    if !scan_completed {
        println!("Triggering manual rescan...");
        if let Ok(mut file_picker_guard) = FILE_PICKER.write()
            && let Some(ref mut picker) = *file_picker_guard
        {
            match picker.trigger_rescan() {
                Ok(_) => println!("Manual rescan completed"),
                Err(e) => println!("Manual rescan failed: {:?}", e),
            }
        }
    }

    let initial_file_count = {
        let file_picker_guard = FILE_PICKER.read()?;
        if let Some(ref picker) = *file_picker_guard {
            let files = picker.get_files();
            println!("Found {} files in picker", files.len());
            if !files.is_empty() {
                println!("Sample files:");
                for (i, file) in files.iter().take(5).enumerate() {
                    println!("  {}. {}", i + 1, file.relative_path);
                }
            }
            files.len()
        } else {
            println!("No picker found in FILE_PICKER static!");
            0
        }
    };

    println!(
        "📊 Initial scan complete: {} files found",
        initial_file_count
    );

    if initial_file_count == 0 {
        eprintln!("❌ No files found in directory. Cannot proceed with memory test.");
        std::process::exit(1);
    }

    // Record initial memory usage
    let initial_memory = get_memory_usage().unwrap_or(0);
    println!("🧠 Initial memory usage: {}", format_bytes(initial_memory));
    println!();

    // Test queries to cycle through
    let test_queries = vec![
        "rs", "mod", "lib", "main", "test", "src", "lua", "cargo", "toml", "init", "config",
        "util", "file", "picker", "fuzzy", "search", "git", "fn", "struct", "impl", "pub", "use",
        "let", "match", "if", "for", "async", "await", "Result", "Error", "Vec", "String",
        "Option",
    ];

    let mut last_memory_check = Instant::now();
    let mut peak_memory = initial_memory;
    let mut memory_samples = Vec::new();

    println!("🔥 Starting intensive search test...");
    println!("Press Ctrl+C to stop the test");
    println!();
    let mut search_count = 0;

    while search_count < 1000 {
        search_count += 1;

        // Perform search directly on FilePicker
        let query = test_queries[search_count % test_queries.len()];
        let max_results = 50 + (search_count % 100); // Vary result count
        let max_threads = 1 + (search_count % 8); // Vary thread count

        let search_start = Instant::now();
        let (result_count, search_duration) = {
            let file_picker_guard = FILE_PICKER.read().unwrap();
            if let Some(ref picker) = *file_picker_guard {
                let search_result = FilePicker::fuzzy_search(
                    picker.get_files(),
                    query,
                    max_results,
                    max_threads,
                    None,
                    false, // prompt_position not relevant for test
                );
                let duration = search_start.elapsed();
                (search_result.items.len(), duration)
            } else {
                (0, search_start.elapsed())
            }
        };

        search_count += 1;

        // Check memory every 100 searches or every 5 seconds
        let now = Instant::now();
        if (search_count % 100 == 0
            || now.duration_since(last_memory_check) > Duration::from_secs(5))
            && let Ok(current_memory) = get_memory_usage()
        {
            memory_samples.push(current_memory);

            if current_memory > peak_memory {
                peak_memory = current_memory;
            }

            let memory_growth = current_memory.saturating_sub(initial_memory);

            println!(
                "🔍 Search #{}: '{}' -> {} results in {:?} | Memory: {} (+{}) | Peak: {}",
                search_count,
                query,
                result_count,
                search_duration,
                format_bytes(current_memory),
                format_bytes(memory_growth),
                format_bytes(peak_memory)
            );

            last_memory_check = now;

            // Calculate memory growth trend over last 10 samples
            if memory_samples.len() >= 10 {
                let recent_samples = &memory_samples[memory_samples.len() - 10..];
                let first_recent = recent_samples[0];
                let last_recent = recent_samples[recent_samples.len() - 1];

                if last_recent > first_recent {
                    let recent_growth = last_recent - first_recent;
                    if recent_growth > 1024 * 1024 {
                        // More than 1MB growth in recent samples
                        println!(
                            "⚠️  POTENTIAL LEAK: Recent memory growth: {}",
                            format_bytes(recent_growth)
                        );
                    }
                }
            }

            io::stdout().flush().unwrap();
        }

        // Brief pause to prevent overwhelming the system
        if search_count % 10 == 0 {
            thread::sleep(Duration::from_millis(1));
        }

        // Force cleanup test every 1000 searches
        if search_count % 1000 == 0 {
            println!("🧹 Forcing potential cleanup at search #{}", search_count);
            // Force garbage collection by creating and dropping temporary data
            thread::sleep(Duration::from_millis(10));
        }
    }

    Ok(())
}
