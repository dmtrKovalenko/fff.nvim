use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use fff_search::bigram_filter::{BigramFilter, BigramIndexBuilder};

/// Build a realistic bigram index for benchmarking.
/// Simulates a large repo by generating varied content per file.
fn build_test_index(file_count: usize) -> BigramFilter {
    let builder = BigramIndexBuilder::new(file_count);
    let skip_builder = BigramIndexBuilder::new(file_count);

    for i in 0..file_count {
        // Generate varied content so we get a mix of sparse and dense columns
        let content = format!(
            "struct File{i} {{ fn process() {{ let controller = read(path); }} }} // module {i}"
        );
        builder.add_file_content(&skip_builder, i, content.as_bytes());
    }

    let mut index = builder.compress(None);
    let skip_index = skip_builder.compress(Some(12));
    index.set_skip_index(skip_index);
    index
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
    let candidates = match index.query(b"struct") {
        Some(c) => c,
        None => {
            // All bigrams ubiquitous at this size — skip candidate benches
            eprintln!("Skipping is_candidate bench: query returned None (all bigrams ubiquitous)");
            return;
        }
    };

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
        // Pre-generate content so we only measure index building.
        // Short content (~85 bytes/file) exercises the scalar fast path.
        let contents: Vec<String> = (0..file_count)
            .map(|i| {
                format!(
                    "struct File{i} {{ fn process() {{ let controller = read(path); }} }} // mod {i}"
                )
            })
            .collect();

        group.bench_with_input(
            BenchmarkId::new("short_content", file_count),
            &file_count,
            |b, &fc| {
                b.iter(|| {
                    let builder = BigramIndexBuilder::new(fc);
                    let skip_builder = BigramIndexBuilder::new(fc);
                    for (i, content) in contents.iter().enumerate() {
                        builder.add_file_content(&skip_builder, i, content.as_bytes());
                    }
                    let index = builder.compress(None);
                    black_box(index.columns_used())
                });
            },
        );

        // Long content (~4 KB/file) exercises the SIMD pre-pass path.
        // Build a realistic-looking source-like blob by repeating snippets.
        let long_contents: Vec<String> = (0..file_count)
            .map(|i| {
                let mut s = String::with_capacity(4096);
                for j in 0..50 {
                    s.push_str(&format!(
                        "pub fn handler_{i}_{j}(ctx: &Context) -> Result<Response, Error> {{\n"
                    ));
                    s.push_str("    let parsed = ctx.parse()?;\n");
                    s.push_str("    let validated = parsed.validate()?;\n");
                    s.push_str(&format!("    ctx.respond(validated, {}).await\n", j));
                    s.push_str("}\n\n");
                }
                s
            })
            .collect();

        group.bench_with_input(
            BenchmarkId::new("long_content", file_count),
            &file_count,
            |b, &fc| {
                b.iter(|| {
                    let builder = BigramIndexBuilder::new(fc);
                    let skip_builder = BigramIndexBuilder::new(fc);
                    for (i, content) in long_contents.iter().enumerate() {
                        builder.add_file_content(&skip_builder, i, content.as_bytes());
                    }
                    let index = builder.compress(None);
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
