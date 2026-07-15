use criterion::{Criterion, criterion_group, criterion_main};
use fff_search::file_picker::{FilePicker, FilePickerOptions};
use fff_search::{GrepMode, GrepSearchOptions, parse_grep_query};
use std::io::Write;

/// Synthetic repo: half the files contain the needle on every line (stresses
/// the per-match find/highlight path), half are pure noise (stresses the
/// whole-file prefilter path).
fn setup_repo(dir: &std::path::Path) {
    for i in 0..400 {
        let mut f = std::fs::File::create(dir.join(format!("match_{i}.rs"))).unwrap();
        for j in 0..100 {
            writeln!(
                f,
                "fn handle_{j}() {{ let controller = Controller::new({j}); controller.run(); }}"
            )
            .unwrap();
        }
    }
    for i in 0..400 {
        let mut f = std::fs::File::create(dir.join(format!("noise_{i}.rs"))).unwrap();
        for j in 0..100 {
            writeln!(
                f,
                "fn compute_{j}() {{ let value = {j} * 42; process(value); }}"
            )
            .unwrap();
        }
    }
}

fn options(mode: GrepMode) -> GrepSearchOptions {
    GrepSearchOptions {
        // Force a full scan of every file so we measure matcher/sink work,
        // not pagination early-exit.
        page_limit: usize::MAX,
        max_matches_per_file: 0,
        mode,
        ..Default::default()
    }
}

fn bench_grep(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    setup_repo(dir.path());

    let mut picker = FilePicker::new(FilePickerOptions {
        base_path: dir.path().to_str().unwrap().into(),
        watch: false,
        ..Default::default()
    })
    .unwrap();
    picker.collect_files().unwrap();
    assert_eq!(picker.get_files().len(), 800);

    let mut group = c.benchmark_group("grep_e2e");
    group.sample_size(30);

    // Case-sensitive, 40k matched lines: hottest find_at/highlight path
    let query = parse_grep_query("Controller");
    let opts = options(GrepMode::PlainText);
    group.bench_function("plain_case_sensitive_many_matches", |b| {
        b.iter(|| {
            let r = picker.grep(&query, &opts);
            assert_eq!(r.files_with_matches, 400);
            std::hint::black_box(r.matches.len())
        });
    });

    // Case-insensitive (SIMD folding path), 120k matched spans
    let query = parse_grep_query("controller");
    group.bench_function("plain_case_insensitive_many_matches", |b| {
        b.iter(|| {
            let r = picker.grep(&query, &opts);
            assert_eq!(r.files_with_matches, 400);
            std::hint::black_box(r.matches.len())
        });
    });

    // No matches anywhere: whole-file prefilter dominates
    let query = parse_grep_query("Qqzyx");
    group.bench_function("plain_no_matches", |b| {
        b.iter(|| {
            let r = picker.grep(&query, &opts);
            assert_eq!(r.files_with_matches, 0);
            std::hint::black_box(r.total_files_searched)
        });
    });

    // Regex mode: must be unaffected by NeedleFinder changes
    let query = parse_grep_query("Contr[a-z]+ller");
    let regex_opts = options(GrepMode::Regex);
    group.bench_function("regex_many_matches", |b| {
        b.iter(|| {
            let r = picker.grep(&query, &regex_opts);
            assert_eq!(r.files_with_matches, 400);
            std::hint::black_box(r.matches.len())
        });
    });

    group.finish();
}

criterion_group!(benches, bench_grep);
criterion_main!(benches);
