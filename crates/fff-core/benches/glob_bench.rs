//! Compare three glob-matching strategies for `match_glob_pattern` in constraints.rs:
//!
//! 1. Current: `zlob_match_paths` -> collect `as_ptr()` into AHashSet, filter paths
//!    by pointer to recover indices.
//! 2. Free fn: `zlob_match_paths_indices` (added in zlob 1.4) — indices direct from C.
//! 3. Compiled: `ZlobPattern::compile` + `match_indices` — same indices path, but with
//!    a precompiled pattern (reusable). For one-shot it should match (2); the win
//!    appears if the pattern is reused (chunked / repeated calls).
use ahash::AHashSet;
use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use zlob::{ZlobFlags, ZlobPattern, zlob_match_paths, zlob_match_paths_indices};

fn make_paths(n: usize) -> Vec<String> {
    let exts = ["rs", "ts", "lua", "md", "toml", "go", "py", "c", "h", "txt"];
    let dirs = [
        "src/core",
        "src/ui",
        "crates/fff-core/src",
        "lua/fff",
        "tests/integration",
        "vendor/lib",
        "node_modules/foo/bar",
        "docs/internal",
    ];
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let dir = dirs[i % dirs.len()];
        let ext = exts[i % exts.len()];
        out.push(format!("{dir}/file_{i}.{ext}"));
    }
    out
}

fn current_impl(pattern: &str, paths: &[&str]) -> AHashSet<usize> {
    let Ok(Some(matches)) = zlob_match_paths(pattern, paths, ZlobFlags::RECOMMENDED) else {
        return AHashSet::new();
    };
    let matched_set: AHashSet<usize> = matches.iter().map(|s| s.as_ptr() as usize).collect();
    paths
        .iter()
        .enumerate()
        .filter(|(_, p)| matched_set.contains(&(p.as_ptr() as usize)))
        .map(|(i, _)| i)
        .collect()
}

fn indices_free_fn(pattern: &str, paths: &[&str]) -> AHashSet<usize> {
    let Ok(hits) = zlob_match_paths_indices(pattern, paths, ZlobFlags::RECOMMENDED) else {
        return AHashSet::new();
    };
    hits.to_iter().collect()
}

fn compiled_pattern(pattern: &str, paths: &[&str]) -> AHashSet<usize> {
    let Ok(p) = ZlobPattern::compile(pattern, ZlobFlags::RECOMMENDED) else {
        return AHashSet::new();
    };
    let Ok(hits) = p.match_indices(paths, ZlobFlags::RECOMMENDED) else {
        return AHashSet::new();
    };
    hits.to_iter().collect()
}

fn bench_glob_strategies(c: &mut Criterion) {
    let path_counts = [1_000usize, 10_000, 100_000];
    let patterns: &[(&str, &str)] = &[
        ("ext_rs", "**/*.rs"),
        ("dir_glob", "src/**/*.{ts,lua}"),
        ("literal_seg", "**/node_modules/**"),
        ("brace_multi", "**/*.{rs,ts,lua,md}"),
    ];

    for &count in &path_counts {
        let owned = make_paths(count);
        let paths: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();

        let mut group = c.benchmark_group(format!("glob_{count}"));
        group.sample_size(50);

        for &(name, pat) in patterns {
            let id_curr = BenchmarkId::new("current_ptr_trick", name);
            group.bench_with_input(id_curr, &pat, |b, &pat| {
                b.iter(|| {
                    let r = current_impl(black_box(pat), black_box(&paths));
                    black_box(r);
                });
            });

            let id_idx = BenchmarkId::new("match_indices_fn", name);
            group.bench_with_input(id_idx, &pat, |b, &pat| {
                b.iter(|| {
                    let r = indices_free_fn(black_box(pat), black_box(&paths));
                    black_box(r);
                });
            });

            let id_comp = BenchmarkId::new("compiled_pattern", name);
            group.bench_with_input(id_comp, &pat, |b, &pat| {
                b.iter(|| {
                    let r = compiled_pattern(black_box(pat), black_box(&paths));
                    black_box(r);
                });
            });
        }

        group.finish();
    }
}

/// Hot-loop: pattern compiled ONCE, matched many times against fresh path slices.
/// Models a hypothetical change where we cache compiled patterns across calls.
fn bench_compiled_reuse(c: &mut Criterion) {
    let owned = make_paths(10_000);
    let paths: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
    let pat = "**/*.{rs,ts,lua,md}";

    let mut group = c.benchmark_group("glob_reuse_10k");
    group.sample_size(100);

    group.bench_function("recompile_each_time", |b| {
        b.iter(|| {
            let p = ZlobPattern::compile(black_box(pat), ZlobFlags::RECOMMENDED).unwrap();
            let hits = p
                .match_indices(black_box(&paths), ZlobFlags::RECOMMENDED)
                .unwrap();
            black_box(hits.len());
        });
    });

    let compiled = ZlobPattern::compile(pat, ZlobFlags::RECOMMENDED).unwrap();
    group.bench_function("reuse_compiled", |b| {
        b.iter(|| {
            let hits = compiled
                .match_indices(black_box(&paths), ZlobFlags::RECOMMENDED)
                .unwrap();
            black_box(hits.len());
        });
    });

    group.finish();
}

/// End-to-end: build the lookup AND iterate items checking membership, modeling the
/// real call shape in `apply_constraints` (filter loop reads the result for every item).
fn bench_full_pipeline(c: &mut Criterion) {
    bench_full_pipeline_size(c, 100_000);
    bench_full_pipeline_size(c, 500_000);
}

fn bench_full_pipeline_size(c: &mut Criterion, count: usize) {
    let owned = make_paths(count);
    let paths: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
    let pat = "**/*.{rs,ts,lua,md}";

    let mut group = c.benchmark_group(format!("glob_full_pipeline_{count}"));
    group.sample_size(50);

    // (A) current: indices -> AHashSet -> per-item set.contains
    group.bench_function("indices_to_ahashset_then_filter", |b| {
        b.iter(|| {
            let hits =
                zlob_match_paths_indices(black_box(pat), &paths, ZlobFlags::RECOMMENDED).unwrap();
            let set: AHashSet<usize> = hits.to_iter().collect();
            let count = (0..paths.len()).filter(|i| set.contains(i)).count();
            black_box(count);
        });
    });

    // (B) indices -> Vec<bool> bitmap -> per-item array lookup
    group.bench_function("indices_to_bitmap_then_filter", |b| {
        b.iter(|| {
            let hits =
                zlob_match_paths_indices(black_box(pat), &paths, ZlobFlags::RECOMMENDED).unwrap();
            let mut mask = vec![false; paths.len()];
            for i in hits.to_iter() {
                mask[i] = true;
            }
            let count = (0..paths.len()).filter(|&i| mask[i]).count();
            black_box(count);
        });
    });

    // (C) compiled pattern + per-item matches() inside the filter loop. No batch.
    group.bench_function("compiled_per_item_matches", |b| {
        b.iter(|| {
            let p = ZlobPattern::compile(black_box(pat), ZlobFlags::RECOMMENDED).unwrap();
            let count = paths.iter().filter(|path| p.matches_default(path)).count();
            black_box(count);
        });
    });

    // (D) compiled pattern + chunked batch -> Vec<bool> bitmap. Best of both:
    //     SIMD batch wins inside chunks, no global allocation pressure, O(1) lookup.
    group.bench_function("compiled_chunked_to_bitmap", |b| {
        b.iter(|| {
            let p = ZlobPattern::compile(black_box(pat), ZlobFlags::RECOMMENDED).unwrap();
            let mut mask = vec![false; paths.len()];
            for (chunk_idx, chunk) in paths.chunks(512).enumerate() {
                let base = chunk_idx * 512;
                let hits = p.match_indices(chunk, ZlobFlags::RECOMMENDED).unwrap();
                for i in hits.to_iter() {
                    mask[base + i] = true;
                }
            }
            let count = (0..paths.len()).filter(|&i| mask[i]).count();
            black_box(count);
        });
    });

    // (E') indices -> bit-packed Vec<u64> -> per-item bit test
    group.bench_function("indices_to_bitset_then_filter", |b| {
        b.iter(|| {
            let hits =
                zlob_match_paths_indices(black_box(pat), &paths, ZlobFlags::RECOMMENDED).unwrap();
            let words = paths.len().div_ceil(64);
            let mut bits = vec![0u64; words];
            for i in hits.to_iter() {
                bits[i >> 6] |= 1u64 << (i & 63);
            }
            let count = (0..paths.len())
                .filter(|&i| (bits[i >> 6] >> (i & 63)) & 1 == 1)
                .count();
            black_box(count);
        });
    });

    // (E) (D) but larger chunk
    group.bench_function("compiled_chunked_4096_to_bitmap", |b| {
        b.iter(|| {
            let p = ZlobPattern::compile(black_box(pat), ZlobFlags::RECOMMENDED).unwrap();
            let mut mask = vec![false; paths.len()];
            for (chunk_idx, chunk) in paths.chunks(4096).enumerate() {
                let base = chunk_idx * 4096;
                let hits = p.match_indices(chunk, ZlobFlags::RECOMMENDED).unwrap();
                for i in hits.to_iter() {
                    mask[base + i] = true;
                }
            }
            let count = (0..paths.len()).filter(|&i| mask[i]).count();
            black_box(count);
        });
    });

    group.finish();
}

/// Mixed-constraint pipeline: glob + ext. Compare pre-pass batch (current) vs
/// inline `ZlobPattern::matches` after the cheap ext check rejects items.
///
/// Variables: ext rejection rate. Extreme cases reveal where each strategy wins.
fn bench_mixed_pipeline(c: &mut Criterion) {
    let count = 100_000;
    let owned = make_paths(count);
    let paths: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
    let glob_pat = "**/*.{rs,ts,lua,md}";

    // 4 ext sets: from very selective (1/10 paths kept) to permissive (kept all).
    let scenarios: &[(&str, &[&str])] = &[
        ("ext_1of10", &["rs"]),
        ("ext_4of10", &["rs", "ts", "lua", "md"]),
        (
            "ext_8of10",
            &["rs", "ts", "lua", "md", "toml", "go", "py", "c"],
        ),
        (
            "ext_all",
            &["rs", "ts", "lua", "md", "toml", "go", "py", "c", "h", "txt"],
        ),
    ];

    fn ext_match(name: &str, exts: &[&str]) -> bool {
        exts.iter().any(|e| {
            let bytes = name.as_bytes();
            let elen = e.len();
            bytes.len() > elen + 1
                && bytes[bytes.len() - elen - 1] == b'.'
                && bytes[bytes.len() - elen..].eq_ignore_ascii_case(e.as_bytes())
        })
    }

    let mut group = c.benchmark_group("glob_mixed_100k");
    group.sample_size(50);

    for &(name, exts) in scenarios {
        // (A) PRE-PASS: build bitmap for ALL paths, then per-item ext-then-bitmap.
        let id_pre = BenchmarkId::new("prepass_bitmap", name);
        group.bench_with_input(id_pre, &exts, |b, &exts| {
            b.iter(|| {
                let hits =
                    zlob_match_paths_indices(black_box(glob_pat), &paths, ZlobFlags::RECOMMENDED)
                        .unwrap();
                let mut mask = vec![false; paths.len()];
                for i in hits.to_iter() {
                    mask[i] = true;
                }
                let count = paths
                    .iter()
                    .enumerate()
                    .filter(|&(_, p)| ext_match(p, exts))
                    .filter(|&(i, _)| mask[i])
                    .count();
                black_box(count);
            });
        });

        // (B) INLINE: compile once, per-item ext check first, then matches() only on survivors.
        let id_inline = BenchmarkId::new("inline_compiled", name);
        group.bench_with_input(id_inline, &exts, |b, &exts| {
            b.iter(|| {
                let p = ZlobPattern::compile(black_box(glob_pat), ZlobFlags::RECOMMENDED).unwrap();
                let count = paths
                    .iter()
                    .filter(|path| ext_match(path, exts))
                    .filter(|path| p.matches_default(path))
                    .count();
                black_box(count);
            });
        });
    }

    group.finish();
}

/// Compare hand-rolled `file_has_extension` byte compare vs compiling extensions
/// into a single brace glob `**/*.{rs,ts,lua,md}` and dispatching through zlob.
/// Both share the same per-item "filter then count" shape.
fn bench_extensions_vs_glob(c: &mut Criterion) {
    let owned = make_paths(100_000);
    let paths: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
    let exts = ["rs", "ts", "lua", "md"];
    let glob_pat = "**/*.{rs,ts,lua,md}";

    fn ext_match(name: &str, exts: &[&str]) -> bool {
        let bytes = name.as_bytes();
        exts.iter().any(|e| {
            let elen = e.len();
            bytes.len() > elen + 1
                && bytes[bytes.len() - elen - 1] == b'.'
                && bytes[bytes.len() - elen..].eq_ignore_ascii_case(e.as_bytes())
        })
    }

    let mut group = c.benchmark_group("ext_vs_glob_100k");
    group.sample_size(50);

    group.bench_function("file_has_extension_loop", |b| {
        b.iter(|| {
            let count = paths.iter().filter(|p| ext_match(p, &exts)).count();
            black_box(count);
        });
    });

    group.bench_function("compiled_brace_glob_inline", |b| {
        b.iter(|| {
            let p = ZlobPattern::compile(black_box(glob_pat), ZlobFlags::RECOMMENDED).unwrap();
            let count = paths.iter().filter(|path| p.matches_default(path)).count();
            black_box(count);
        });
    });

    group.bench_function("brace_glob_prepass_bitmap", |b| {
        b.iter(|| {
            let hits =
                zlob_match_paths_indices(black_box(glob_pat), &paths, ZlobFlags::RECOMMENDED)
                    .unwrap();
            let mut mask = vec![false; paths.len()];
            for i in hits.to_iter() {
                mask[i] = true;
            }
            let count = (0..paths.len()).filter(|&i| mask[i]).count();
            black_box(count);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_glob_strategies,
    bench_compiled_reuse,
    bench_full_pipeline,
    bench_mixed_pipeline,
    bench_extensions_vs_glob
);
criterion_main!(benches);
