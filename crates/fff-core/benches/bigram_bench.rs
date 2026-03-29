use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use fff_search::types::{BigramFilter, BigramIndexBuilder};

/// Build a realistic bigram index for benchmarking.
/// Simulates a large repo by generating varied content per file.
fn build_test_index(file_count: usize) -> BigramFilter {
    let builder = BigramIndexBuilder::new(file_count);

    for i in 0..file_count {
        // Generate varied content so we get a mix of sparse and dense columns
        let content = format!(
            "struct File{i} {{ fn process() {{ let controller = read(path); }} }} // module {i}"
        );
        builder.add_file_content(i, content.as_bytes());
    }

    builder.compress()
}

fn bench_bigram_query(c: &mut Criterion) {
    let file_counts = [10_000, 100_000, 500_000];

    for &file_count in &file_counts {
        let index = build_test_index(file_count);
        eprintln!(
            "Index ({} files): {} columns",
            file_count,
            index.columns_used(),
        );

        let mut group = c.benchmark_group(format!("bigram_query_{file_count}"));
        group.sample_size(500);

        let queries: &[(&str, &[u8])] = &[
            ("short_2char", b"st"),
            ("medium_6char", b"struct"),
            ("long_14char", b"let controller"),
            ("multi_word", b"fn process"),
        ];

        for (name, query) in queries {
            group.bench_with_input(BenchmarkId::from_parameter(name), query, |b, q| {
                b.iter(|| {
                    let result = index.query(black_box(q));
                    black_box(&result);
                });
            });
        }

        group.finish();
    }
}

fn bench_bigram_is_candidate(c: &mut Criterion) {
    let index = build_test_index(500_000);
    let candidates = index.query(b"struct").unwrap();

    c.bench_function("is_candidate_500k", |b| {
        b.iter(|| {
            let mut count = 0u32;
            for i in 0..500_000 {
                if BigramFilter::is_candidate(black_box(&candidates), i) {
                    count += 1;
                }
            }
            black_box(count)
        });
    });

    c.bench_function("count_candidates_500k", |b| {
        b.iter(|| BigramFilter::count_candidates(black_box(&candidates)));
    });
}

fn bench_bigram_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("bigram_build");
    group.sample_size(10);

    let file_counts = [10_000, 100_000];

    for &file_count in &file_counts {
        // Pre-generate content so we only measure index building
        let contents: Vec<String> = (0..file_count)
            .map(|i| {
                format!(
                    "struct File{i} {{ fn process() {{ let controller = read(path); }} }} // mod {i}"
                )
            })
            .collect();

        group.bench_with_input(
            BenchmarkId::new("build_and_compress", file_count),
            &file_count,
            |b, &fc| {
                b.iter(|| {
                    let builder = BigramIndexBuilder::new(fc);
                    for (i, content) in contents.iter().enumerate() {
                        builder.add_file_content(i, content.as_bytes());
                    }
                    let index = builder.compress();
                    black_box(index.columns_used())
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_bigram_query,
    bench_bigram_is_candidate,
    bench_bigram_build,
);

criterion_main!(benches);
