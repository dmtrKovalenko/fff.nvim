/// Fuzzy grep quality test against ~/dev/lightsource
///
/// Runs queries through the fuzzy grep pipeline and prints results
/// so we can verify match quality.
///
/// Usage:
///   cargo run --release --bin fuzzy_grep_test              # runs default test queries
///   cargo run --release --bin fuzzy_grep_test -- "query"   # runs a single user query
use fff_core::grep::{grep_search, parse_grep_query, GrepMode, GrepSearchOptions};
use fff_core::FileItem;
use std::io::Read;
use std::path::Path;
use std::time::Instant;

fn load_files(base_path: &Path) -> Vec<FileItem> {
    use ignore::WalkBuilder;

    let mut files = Vec::new();

    WalkBuilder::new(base_path)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .follow_links(false)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_some_and(|ft| ft.is_file()))
        .for_each(|entry| {
            let path = entry.path().to_path_buf();
            let relative = pathdiff::diff_paths(&path, base_path).unwrap_or_else(|| path.clone());
            let relative_path = relative.to_string_lossy().into_owned();
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let size = entry.metadata().ok().map_or(0, |m| m.len());
            let is_binary = detect_binary(&path, size);

            files.push(FileItem::new_raw(
                path,
                relative_path,
                file_name,
                size,
                0,
                None,
                is_binary,
            ));
        });

    files
}

fn detect_binary(path: &Path, size: u64) -> bool {
    if size == 0 {
        return false;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut reader = std::io::BufReader::with_capacity(1024, file);
    let mut buf = [0u8; 512];
    let n = reader.read(&mut buf).unwrap_or(0);
    buf[..n].contains(&0)
}

fn run_fuzzy_query(files: &[FileItem], query: &str, label: &str) {
    let options = GrepSearchOptions {
        max_file_size: 10 * 1024 * 1024,
        max_matches_per_file: 200,
        smart_case: true,
        file_offset: 0,
        page_limit: 100, // Get plenty of results
        mode: GrepMode::Fuzzy,
        time_budget_ms: 0, // No time limit — search all files
    };

    let parsed = parse_grep_query(query);
    let start = Instant::now();
    let result = grep_search(files, query, parsed, &options);
    let elapsed = start.elapsed();

    eprintln!("══════════════════════════════════════════════════════════════");
    eprintln!("  Query: \"{}\"  ({})", query, label);
    eprintln!(
        "  Results: {} matches in {} files ({:.2}ms)",
        result.matches.len(),
        result.total_files_searched,
        elapsed.as_secs_f64() * 1000.0,
    );
    eprintln!("══════════════════════════════════════════════════════════════");

    if result.matches.is_empty() {
        eprintln!("  (no matches)\n");
        return;
    }

    // Group by file for readability
    let mut current_file_idx = usize::MAX;
    for (i, m) in result.matches.iter().enumerate() {
        if m.file_index != current_file_idx {
            current_file_idx = m.file_index;
            let file = &result.files[m.file_index];
            eprintln!("\n  ┌─ {}", file.relative_path);
        }

        // Truncate long lines for display
        let display_line = if m.line_content.len() > 100 {
            format!("{}...", &m.line_content[..100])
        } else {
            m.line_content.clone()
        };

        let score_str = m
            .fuzzy_score
            .map(|s| format!("score={}", s))
            .unwrap_or_else(|| "no-score".to_string());

        let offsets_str = if m.match_byte_offsets.is_empty() {
            String::new()
        } else {
            // Show what text fragments are highlighted
            let fragments: Vec<String> = m
                .match_byte_offsets
                .iter()
                .filter_map(|&(s, e)| {
                    m.line_content
                        .get(s as usize..e as usize)
                        .map(|frag| format!("\"{}\"", frag))
                })
                .collect();
            format!(" hl=[{}]", fragments.join(","))
        };

        eprintln!(
            "  │ L{:<5} [{}{}] {}",
            m.line_number,
            score_str,
            offsets_str,
            display_line.trim(),
        );

        // Cap output at 50 lines
        if i >= 49 {
            let remaining = result.matches.len() - 50;
            if remaining > 0 {
                eprintln!("  │ ... and {} more matches", remaining);
            }
            break;
        }
    }
    eprintln!();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let repo_path = std::path::PathBuf::from(
        std::env::var("HOME").unwrap_or_else(|_| "/Users/neogoose".to_string()),
    )
    .join("dev/lightsource");

    if !repo_path.exists() {
        eprintln!("Repository not found at: {:?}", repo_path);
        std::process::exit(1);
    }

    let canonical =
        fff_core::path_utils::canonicalize(&repo_path).expect("Failed to canonicalize path");
    eprintln!("=== Fuzzy Grep Quality Test ===");
    eprintln!("Repository: {:?}\n", canonical);

    eprintln!("Loading files...");
    let load_start = Instant::now();
    let files = load_files(&canonical);
    let non_binary = files.iter().filter(|f| !f.is_binary).count();
    eprintln!(
        "Loaded {} files ({} non-binary) in {:.2}s\n",
        files.len(),
        non_binary,
        load_start.elapsed().as_secs_f64()
    );

    if args.is_empty() {
        // Run default test queries
        run_fuzzy_query(&files, "shcema", "transposition of 'schema'");
        run_fuzzy_query(&files, "SortedMap", "should match SortedArrayMap");
        run_fuzzy_query(
            &files,
            "struct SortedMap",
            "should NOT match SourcingProjectMetadataParts",
        );
    } else {
        // Run user-provided queries
        for query in &args {
            run_fuzzy_query(&files, query, "user query");
        }
    }

    eprintln!("=== Done ===");
}
