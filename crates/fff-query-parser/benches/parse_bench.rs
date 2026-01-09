use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use fff_query_parser::*;

fn bench_parse_simple(c: &mut Criterion) {
    let parser = QueryParser::default();

    c.bench_function("parse_simple_text", |b| {
        b.iter(|| parser.parse(black_box("hello world")));
    });

    c.bench_function("parse_extension", |b| {
        b.iter(|| parser.parse(black_box("*.rs")));
    });

    c.bench_function("parse_text_with_extension", |b| {
        b.iter(|| parser.parse(black_box("name *.rs")));
    });
}

fn bench_parse_complex(c: &mut Criterion) {
    let parser = QueryParser::default();

    c.bench_function("parse_complex_mixed", |b| {
        b.iter(|| parser.parse(black_box("src name *.rs !test /lib/ status:modified")));
    });

    c.bench_function("parse_glob", |b| {
        b.iter(|| parser.parse(black_box("**/*.rs")));
    });

    c.bench_function("parse_multiple_constraints", |b| {
        b.iter(|| parser.parse(black_box("*.rs *.toml *.md !test !node_modules /src/")));
    });
}

fn bench_parse_realistic_queries(c: &mut Criterion) {
    let parser = QueryParser::default();

    let queries = vec![
        "file",
        "test",
        "mod.rs",
        "src/*.rs",
        "lib test",
        "*.rs !test",
        "src/lib/*.rs",
        "/src/ name",
        "status:modified *.rs",
        "type:rust test !node_modules",
    ];

    let mut group = c.benchmark_group("realistic_queries");
    for query in queries.iter() {
        group.throughput(Throughput::Bytes(query.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(query), query, |b, q| {
            b.iter(|| parser.parse(black_box(q)));
        });
    }
    group.finish();
}

fn bench_parse_various_lengths(c: &mut Criterion) {
    let parser = QueryParser::default();

    let short = "*.rs";
    let medium = "src name *.rs !test";
    let long = "src lib test name *.rs *.toml !node_modules !test /src/ /lib/ status:modified";
    let very_long =
        "a b c d e f g h i j k l m n o p q r s t u v w x y z *.rs *.toml *.md *.txt *.js";

    let mut group = c.benchmark_group("query_lengths");

    group.throughput(Throughput::Bytes(short.len() as u64));
    group.bench_with_input(BenchmarkId::new("short", short.len()), &short, |b, q| {
        b.iter(|| parser.parse(black_box(q)));
    });

    group.throughput(Throughput::Bytes(medium.len() as u64));
    group.bench_with_input(BenchmarkId::new("medium", medium.len()), &medium, |b, q| {
        b.iter(|| parser.parse(black_box(q)));
    });

    group.throughput(Throughput::Bytes(long.len() as u64));
    group.bench_with_input(BenchmarkId::new("long", long.len()), &long, |b, q| {
        b.iter(|| parser.parse(black_box(q)));
    });

    group.throughput(Throughput::Bytes(very_long.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("very_long", very_long.len()),
        &very_long,
        |b, q| {
            b.iter(|| parser.parse(black_box(q)));
        },
    );

    group.finish();
}

fn bench_config_comparison(c: &mut Criterion) {
    let file_picker = QueryParser::new(FilePickerConfig);
    let grep = QueryParser::new(GrepConfig);

    let query = "src name *.rs !test";

    let mut group = c.benchmark_group("config_comparison");

    group.bench_function("file_picker_config", |b| {
        b.iter(|| file_picker.parse(black_box(query)));
    });

    group.bench_function("grep_config", |b| {
        b.iter(|| grep.parse(black_box(query)));
    });

    group.finish();
}

fn bench_constraint_types(c: &mut Criterion) {
    let parser = QueryParser::default();

    let mut group = c.benchmark_group("constraint_types");

    group.bench_function("extension", |b| {
        b.iter(|| parser.parse(black_box("*.rs")));
    });

    group.bench_function("glob", |b| {
        b.iter(|| parser.parse(black_box("**/*.rs")));
    });

    group.bench_function("exclude", |b| {
        b.iter(|| parser.parse(black_box("!test")));
    });

    group.bench_function("path_segment", |b| {
        b.iter(|| parser.parse(black_box("/src/")));
    });

    group.bench_function("git_status", |b| {
        b.iter(|| parser.parse(black_box("status:modified")));
    });

    group.bench_function("file_type", |b| {
        b.iter(|| parser.parse(black_box("type:rust")));
    });

    group.finish();
}

fn bench_worst_case(c: &mut Criterion) {
    let parser = QueryParser::default();

    // Worst case: many constraints that all need to be checked
    let worst_case = "a b c d e f g h i j k l m n o p q r s t u v w x y z";

    c.bench_function("worst_case_many_text_tokens", |b| {
        b.iter(|| parser.parse(black_box(worst_case)));
    });

    // Many constraints
    let many_constraints = "*.rs *.toml *.md *.txt *.js *.ts *.jsx *.tsx *.vue *.svelte";

    c.bench_function("worst_case_many_constraints", |b| {
        b.iter(|| parser.parse(black_box(many_constraints)));
    });
}

criterion_group!(
    benches,
    bench_parse_simple,
    bench_parse_complex,
    bench_parse_realistic_queries,
    bench_parse_various_lengths,
    bench_config_comparison,
    bench_constraint_types,
    bench_worst_case,
);

criterion_main!(benches);
