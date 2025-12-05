# FFF.nvim Benchmarks

This directory contains Criterion benchmarks for measuring the performance of the FFF.nvim plugin.

## Setup

Place your test repository in `./big-repo` at the project root. The benchmarks will index and search this directory.

```bash
# Example: clone a large repository for testing
git clone https://github.com/torvalds/linux big-repo
# or
git clone https://github.com/rust-lang/rust big-repo
```

## Running Benchmarks

```bash
# Run all benchmarks
cargo bench --bench indexing_and_search

# Run only indexing benchmark
cargo bench --bench indexing_and_search -- indexing

# Run only search benchmarks
cargo bench --bench indexing_and_search -- search

# Run only thread scaling tests
cargo bench --bench indexing_and_search -- thread_scaling

# Run only result limit tests
cargo bench --bench indexing_and_search -- result_limits
```

## Benchmark Groups

### 1. Indexing (`bench_indexing`)
Measures the complete indexing time for the `./big-repo` directory:
- File system scanning
- Git status detection
- Frecency score calculation
- Background watcher initialization

**What it measures:** Time from `FilePicker::new()` until all files are indexed and available.

### 2. Search Queries (`bench_search_queries`)
Tests fuzzy search with different query patterns:
- **short**: `"mod"` - common short query
- **medium**: `"controller"` - medium length query
- **long**: `"user_authentication"` - long specific query
- **typo**: `"contrlr"` - query with typos (tests typo-resistance)
- **partial**: `"src/lib"` - path-like query

**What it measures:** Pure search time (no indexing overhead).

### 3. Thread Scaling (`bench_search_thread_scaling`)
Compares search performance with different thread counts (1, 2, 4, 8).

**What it measures:** How well the parallel search scales with CPU cores.

### 4. Result Limits (`bench_search_result_limits`)
Tests search with different max result counts (10, 50, 100, 500).

**What it measures:** Impact of result limit on search performance.

## Output

### Console Output
Real-time progress with file counts:
```
✓ Indexed 50000 files
✓ Search benchmarks will use 50000 files
```

### HTML Reports
Detailed reports generated in `target/criterion/`:
- View at `target/criterion/report/index.html`
- Includes graphs, statistics, and historical comparisons
- Automatically detects performance regressions

### Comparison
Criterion automatically compares against previous runs:
```
indexing/index_big_repo
                        time:   [1.234 s 1.250 s 1.267 s]
                        change: [-5.23% -3.45% -1.67%] (p = 0.001 < 0.05)
                        Performance has improved.
```

## Profiling Tips

### For detailed profiling, use:
```bash
# With flamegraph (install cargo-flamegraph first)
cargo flamegraph --bench indexing_and_search -- --bench indexing

# With perf
perf record --call-graph dwarf cargo bench --bench indexing_and_search -- indexing
perf report
```

### Enable debug output:
```bash
RUST_LOG=debug cargo bench --bench indexing_and_search
```

## Customizing Benchmarks

Edit `benches/indexing_and_search.rs` to:
- Change sample size: `group.sample_size(N)`
- Change measurement time: `group.measurement_time(Duration::from_secs(N))`
- Add custom queries or test scenarios
- Adjust thread counts or result limits

## Performance Expectations

Typical results for a 50k file repository:
- **Indexing**: 1-3 seconds (depends on disk speed and git status)
- **Search**: 5-50ms (depends on query complexity and match count)
- **Thread scaling**: ~2-4x speedup from 1→4 threads
- **Result limits**: Minimal impact (unless extremely large)

## Troubleshooting

### "Indexed 0 files"
The benchmark now properly waits for async indexing. If you still see this:
- Ensure `./big-repo` exists and contains files
- Check that files aren't all gitignored
- Increase timeout in `wait_for_scan_completion()`

### Slow indexing
- Check if git repository is very large (git status can be slow)
- Disable git integration temporarily by testing on non-git directory
- Profile with `perf` or flamegraph to identify bottleneck

### Inconsistent results
- Close other applications to reduce system noise
- Increase sample size for more stable measurements
- Run benchmarks multiple times to establish baseline
