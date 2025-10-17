use fff_nvim::{FILE_PICKER, file_picker::FilePicker};
use std::env;
use std::thread;
use std::time::Duration;

fn get_mem_stat() -> Result<(usize, usize, usize), Box<dyn std::error::Error>> {
    // Use system memory info since jemalloc-ctl conflicts
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let pid = std::process::id();
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()?;

        let rss_str = String::from_utf8(output.stdout)?;
        let rss_kb: usize = rss_str.trim().parse()?;
        let rss_bytes = rss_kb * 1024;
        Ok((rss_bytes, rss_bytes, rss_bytes))
    }

    #[cfg(target_os = "linux")]
    {
        let pid = std::process::id();
        let status_path = format!("/proc/{}/status", pid);
        let content = std::fs::read_to_string(status_path)?;

        for line in content.lines() {
            if line.starts_with("VmRSS:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Ok(rss_kb) = parts[1].parse::<usize>()
                    && parts.len() >= 2
                {
                    let rss_bytes = rss_kb * 1024;
                    return Ok((rss_bytes, rss_bytes, rss_bytes));
                }
            }
        }
        Err("Could not find VmRSS in /proc/pid/status".into())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok((0, 0, 0))
    }
}

fn format_bytes(bytes: usize) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_index])
}

fn test_search_memory_pattern(
    name: &str,
    iterations: usize,
    query_pattern: impl Fn(usize) -> String,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("🧪 {}", name);

    let (initial_allocated, initial_active, initial_resident) = get_mem_stat()?;
    println!(
        "  Initial - Allocated: {}, Active: {}, Resident: {}",
        format_bytes(initial_allocated),
        format_bytes(initial_active),
        format_bytes(initial_resident)
    );

    let mut max_allocated = initial_allocated;
    let mut max_active = initial_active;
    let mut max_resident = initial_resident;

    for i in 0..iterations {
        let query = query_pattern(i);

        let (result_count, _total_matched) = {
            let file_picker_guard = FILE_PICKER.read().unwrap();
            if let Some(ref picker) = *file_picker_guard {
                let search_result = FilePicker::fuzzy_search(
                    picker.get_files(),
                    &query,
                    50 + (i % 50), // Vary result count
                    1 + (i % 4),   // Vary thread count
                    None,
                    false, // prompt_position not relevant for test
                );
                (search_result.items.len(), search_result.total_matched)
            } else {
                continue;
            }
        };

        if i % 100 == 0 {
            let (allocated, active, resident) = get_mem_stat()?;
            max_allocated = max_allocated.max(allocated);
            max_active = max_active.max(active);
            max_resident = max_resident.max(resident);

            if i % 500 == 0 {
                println!(
                    "    Iteration {}: {} results, Allocated: {} (+{})",
                    i,
                    result_count,
                    format_bytes(allocated),
                    format_bytes(allocated.saturating_sub(initial_allocated))
                );
            }
        }

        if i % 200 == 199 {
            thread::sleep(Duration::from_millis(1));
        }
    }

    let (final_allocated, final_active, final_resident) = get_mem_stat()?;

    println!(
        "  Final - Allocated: {} (+{}), Active: {} (+{}), Resident: {} (+{})",
        format_bytes(final_allocated),
        format_bytes(final_allocated.saturating_sub(initial_allocated)),
        format_bytes(final_active),
        format_bytes(final_active.saturating_sub(initial_active)),
        format_bytes(final_resident),
        format_bytes(final_resident.saturating_sub(initial_resident))
    );

    println!(
        "  Peak - Allocated: {} (+{}), Active: {} (+{}), Resident: {} (+{})",
        format_bytes(max_allocated),
        format_bytes(max_allocated.saturating_sub(initial_allocated)),
        format_bytes(max_active),
        format_bytes(max_active.saturating_sub(initial_active)),
        format_bytes(max_resident),
        format_bytes(max_resident.saturating_sub(initial_resident))
    );

    let growth_per_search =
        (final_allocated.saturating_sub(initial_allocated)) as f64 / iterations as f64;
    println!(
        "  Average growth per search: {:.2} bytes",
        growth_per_search
    );
    println!();

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let base_path = if args.len() > 1 {
        args[1].clone()
    } else {
        env::current_dir()?.to_str().unwrap_or(".").to_string()
    };

    println!("🧪 FFF.nvim Jemalloc Memory Profiler");
    println!("====================================");
    println!("Test directory: {}", base_path);
    println!();

    // Initialize FilePicker
    {
        let mut file_picker_guard = FILE_PICKER.write().unwrap();
        if file_picker_guard.is_none() {
            println!("Initializing FilePicker...");
            *file_picker_guard = Some(FilePicker::new(base_path.clone())?);
        }
    }

    // Wait for initial scan
    println!("Waiting for file scan...");
    loop {
        if let Ok(file_picker_guard) = FILE_PICKER.read()
            && let Some(ref picker) = *file_picker_guard
            && !picker.is_scan_active()
            && !picker.get_files().is_empty()
        {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    let file_count = {
        let file_picker_guard = FILE_PICKER.read()?;
        file_picker_guard.as_ref().unwrap().get_files().len()
    };

    println!("📊 Found {} files", file_count);

    let (initial_allocated, initial_active, initial_resident) = get_mem_stat()?;
    println!("🧠 Baseline memory:");
    println!("   Allocated: {}", format_bytes(initial_allocated));
    println!("   Active: {}", format_bytes(initial_active));
    println!("   Resident: {}", format_bytes(initial_resident));
    println!();

    // Test different memory patterns

    // 1. Repeated same query - should have minimal growth if caching works
    test_search_memory_pattern("Same Query Repeated (1000x)", 1000, |_| "test".to_string())?;

    // 2. Cycling through different queries
    test_search_memory_pattern("Cycling Queries (1000x)", 1000, |i| {
        let queries = [
            "test", "main", "lib", "src", "mod", "file", "picker", "fuzzy", "search",
        ];
        queries[i % queries.len()].to_string()
    })?;

    // 3. Unique queries each time - worst case for any caching
    test_search_memory_pattern("Unique Queries (500x)", 500, |i| {
        format!("unique_query_{}", i)
    })?;

    // 4. Queries that return many results
    test_search_memory_pattern(
        "High Result Count (500x)",
        500,
        |_| "a".to_string(), // Single character likely to match many files
    )?;

    // 5. Queries with no results
    test_search_memory_pattern("No Results (500x)", 500, |_| {
        "zzzz_no_match_expected".to_string()
    })?;

    // 6. Long intensive test
    test_search_memory_pattern("Long Intensive Test (2000x)", 2000, |i| {
        let patterns = [
            "rs", "lua", "toml", "mod", "lib", "main", "test", "src", "file",
        ];
        format!("{}{}", patterns[i % patterns.len()], i % 100)
    })?;

    let (final_allocated, final_active, final_resident) = get_mem_stat()?;

    println!("🏁 Final Memory Stats:");
    println!(
        "   Allocated: {} (growth: {})",
        format_bytes(final_allocated),
        format_bytes(final_allocated.saturating_sub(initial_allocated))
    );
    println!(
        "   Active: {} (growth: {})",
        format_bytes(final_active),
        format_bytes(final_active.saturating_sub(initial_active))
    );
    println!(
        "   Resident: {} (growth: {})",
        format_bytes(final_resident),
        format_bytes(final_resident.saturating_sub(initial_resident))
    );

    let total_searches = 5500;
    let avg_growth =
        (final_allocated.saturating_sub(initial_allocated)) as f64 / total_searches as f64;
    println!("   Average growth per search: {:.2} bytes", avg_growth);

    Ok(())
}
