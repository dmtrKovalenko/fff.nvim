use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::constraints::Constrainable;
use crate::query_tracker::QueryMatchEntry;
use ahash::AHashMap;
use fff_query_parser::{FFFQuery, FuzzyQuery, Location};

/// Cached file contents — mmap on Unix, heap buffer on Windows.
///
/// On Windows, memory-mapped files hold the file handle open and prevent
/// editors from saving (writing/replacing) those files. Reading into a
/// `Vec<u8>` releases the handle immediately after the read completes.
///
/// The `Buffer` variant is also used on Unix for temporary (uncached) reads
/// where the mmap/munmap syscall overhead exceeds the cost of a heap copy.
#[derive(Debug)]
#[allow(dead_code)] // variants are conditionally used per platform
pub enum FileContent {
    #[cfg(not(target_os = "windows"))]
    Mmap(memmap2::Mmap),
    Buffer(Vec<u8>),
}

impl std::ops::Deref for FileContent {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            #[cfg(not(target_os = "windows"))]
            FileContent::Mmap(m) => m,
            FileContent::Buffer(b) => b,
        }
    }
}

/// A single indexed file with metadata, frecency scores, and lazy content cache.
///
/// File contents are initialized lazily on the first grep access and cached for
/// subsequent searches. On Unix, uses mmap backed by the kernel page cache. On
/// Windows, reads into a heap buffer to avoid holding file handles open.
///
/// Thread-safety: `OnceLock` provides lock-free reads after initialization.
/// Each file is only searched by one rayon worker at a time via `par_iter`.
#[derive(Debug)]
pub struct FileItem {
    pub path: PathBuf,
    pub relative_path: String,
    pub file_name: String,
    pub size: u64,
    pub modified: u64,
    pub access_frecency_score: i32,
    pub modification_frecency_score: i32,
    pub total_frecency_score: i32,
    pub git_status: Option<git2::Status>,
    pub is_binary: bool,
    /// Tombstone flag — file was deleted but index slot is preserved so
    /// bigram indices for other files stay valid.
    pub is_deleted: bool,
    /// Lazily-initialized file contents for grep.
    /// Initialized on first grep access via `OnceLock`; lock-free on subsequent reads.
    content: OnceLock<FileContent>,
}

impl Clone for FileItem {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            relative_path: self.relative_path.clone(),
            file_name: self.file_name.clone(),
            size: self.size,
            modified: self.modified,
            access_frecency_score: self.access_frecency_score,
            modification_frecency_score: self.modification_frecency_score,
            total_frecency_score: self.total_frecency_score,
            git_status: self.git_status,
            is_binary: self.is_binary,
            is_deleted: self.is_deleted,
            // Don't clone the content — the clone lazily re-creates it on demand
            content: OnceLock::new(),
        }
    }
}

/// File content that is either borrowed from the persistent cache or owned
/// from a temporary mmap. Dereferences to `&[u8]` so callers can use it
/// transparently.
///
/// On Unix the uncached variant holds a temporary `memmap2::Mmap` that is
/// backed by the kernel page cache — same zero-copy benefit as the cached
/// path, but the mapping is released (munmap) as soon as this value is
/// dropped instead of being retained for the lifetime of the `FileItem`.
pub enum FileContentRef<'a> {
    /// Content is stored in the `FileItem`'s `OnceLock` cache (fast path).
    Cached(&'a [u8]),
    /// Temporary mmap (Unix) / heap buffer (Windows) created because the
    /// persistent cache budget was exceeded. Unmapped on drop.
    Temp(FileContent),
}

impl std::ops::Deref for FileContentRef<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            FileContentRef::Cached(s) => s,
            FileContentRef::Temp(c) => c,
        }
    }
}

impl FileItem {
    /// Create a new `FileItem` with all fields specified and an empty (not yet loaded) mmap.
    pub fn new_raw(
        path: PathBuf,
        relative_path: String,
        file_name: String,
        size: u64,
        modified: u64,
        git_status: Option<git2::Status>,
        is_binary: bool,
    ) -> Self {
        Self {
            path,
            relative_path,
            file_name,
            size,
            modified,
            access_frecency_score: 0,
            modification_frecency_score: 0,
            total_frecency_score: 0,
            git_status,
            is_binary,
            is_deleted: false,
            content: OnceLock::new(),
        }
    }

    /// Invalidate the cached content so the next `get_content()` call creates a fresh one.
    ///
    /// Call this when the background watcher detects that the file has been modified.
    /// On Unix, a file that is truncated while mapped can cause SIGBUS. On Windows,
    /// the stale buffer simply won't reflect the new contents. In both cases,
    /// invalidating ensures a fresh read on the next access.
    pub fn invalidate_mmap(&mut self, budget: &ContentCacheBudget) {
        if self.content.get().is_some() {
            budget.cached_count.fetch_sub(1, Ordering::Relaxed);
            budget.cached_bytes.fetch_sub(self.size, Ordering::Relaxed);
        }

        self.content = OnceLock::new();
    }

    /// Get the cached file contents or lazily load and cache them.
    ///
    /// Returns `None` if the file is too large, empty, can't be opened, **or
    /// the cache budget is exhausted**. Callers that need content regardless
    /// of the budget should use [`get_content_for_search`].
    ///
    /// After the first call, this is lock-free (just an atomic load + pointer deref).
    pub fn get_content(&self, budget: &ContentCacheBudget) -> Option<&[u8]> {
        if let Some(content) = self.content.get() {
            return Some(content);
        }

        let max_file_size = budget.max_file_size;
        if self.size == 0 || self.size > max_file_size {
            return None;
        }

        // Check cache budget before creating a new persistent cache entry.
        let count = budget.cached_count.load(Ordering::Relaxed);
        let bytes = budget.cached_bytes.load(Ordering::Relaxed);
        let max_files = budget.max_files;
        let max_bytes = budget.max_bytes;
        if count >= max_files || bytes + self.size > max_bytes {
            return None;
        }

        let content = load_file_content(&self.path, self.size)?;
        let result = self.content.get_or_init(|| content);

        // Bump counters. Slight over-count under races is fine — the budget
        // is a soft limit and the overshoot is bounded by rayon thread count.
        budget.cached_count.fetch_add(1, Ordering::Relaxed);
        budget.cached_bytes.fetch_add(self.size, Ordering::Relaxed);

        Some(result)
    }

    /// Get file content for searching — **always returns content** for eligible
    /// files, even when the persistent cache budget is exhausted.
    ///
    /// Tries the `OnceLock` cache first (fast path). If the cache is full,
    /// falls back to a temporary mmap that is unmapped when the returned
    /// [`FileContentRef`] is dropped — no persistent kernel resources retained.
    #[inline]
    pub fn get_content_for_search<'a>(
        &'a self,
        budget: &ContentCacheBudget,
    ) -> Option<FileContentRef<'a>> {
        if let Some(cached) = self.get_content(budget) {
            return Some(FileContentRef::Cached(cached));
        }

        // get_content returned None — either ineligible or over budget.
        let max_file_size = budget.max_file_size;
        if self.is_binary || self.size == 0 || self.size > max_file_size {
            return None;
        }

        // Over budget: create a temporary mmap that is unmapped on drop.
        let content = load_file_content(&self.path, self.size)?;
        Some(FileContentRef::Temp(content))
    }
}

/// Maximum number of distinct bigrams tracked in the inverted index.
/// 95 printable ASCII chars (32..=126) after lowercasing → ~70 distinct → 4900 possible.
/// We cap at 5000 to cover all printable bigrams with margin.
/// 5000 columns × 62.5KB (500k files) = 305MB. For 50k files: 30MB.
const MAX_BIGRAM_COLUMNS: usize = 5000;

/// Sentinel value: bigram has no allocated column.
const NO_COLUMN: u32 = u32::MAX;

/// Page size on Apple Silicon is 16KB; on x86-64 it's 4KB.
/// Files smaller than one page waste the remainder when mmapped.
/// Reading them into a heap buffer avoids this overhead.
#[cfg(target_arch = "aarch64")]
const MMAP_THRESHOLD: u64 = 16 * 1024;
#[cfg(not(target_arch = "aarch64"))]
const MMAP_THRESHOLD: u64 = 4 * 1024;

/// Load file contents: small files are read into a heap buffer to avoid
/// mmap page alignment waste; large files use mmap for zero-copy access.
/// On Windows, always uses heap buffer (mmap holds the file handle open).
fn load_file_content(path: &Path, size: u64) -> Option<FileContent> {
    #[cfg(not(target_os = "windows"))]
    {
        if size < MMAP_THRESHOLD {
            let data = std::fs::read(path).ok()?;
            Some(FileContent::Buffer(data))
        } else {
            let file = std::fs::File::open(path).ok()?;
            // SAFETY: The mmap is backed by the kernel page cache and automatically
            // reflects file modifications. The only risk is SIGBUS if the file is
            // truncated while mapped.
            let mmap = unsafe { memmap2::Mmap::map(&file) }.ok()?;
            Some(FileContent::Mmap(mmap))
        }
    }

    #[cfg(target_os = "windows")]
    {
        let _ = size;
        let data = std::fs::read(path).ok()?;
        Some(FileContent::Buffer(data))
    }
}

impl Constrainable for FileItem {
    #[inline]
    fn relative_path(&self) -> &str {
        &self.relative_path
    }

    #[inline]
    fn file_name(&self) -> &str {
        &self.file_name
    }

    #[inline]
    fn git_status(&self) -> Option<git2::Status> {
        self.git_status
    }
}

#[derive(Debug, Clone, Default)]
pub struct Score {
    pub total: i32,
    pub base_score: i32,
    pub filename_bonus: i32,
    pub special_filename_bonus: i32,
    pub frecency_boost: i32,
    pub git_status_boost: i32,
    pub distance_penalty: i32,
    pub current_file_penalty: i32,
    pub combo_match_boost: i32,
    pub exact_match: bool,
    pub match_type: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct PaginationArgs {
    pub offset: usize,
    pub limit: usize,
}

impl Default for PaginationArgs {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 100,
        }
    }
}

/// Context for scoring files during search.
///
/// The `query` field contains the pre-parsed query with constraints,
/// fuzzy parts, and location information. Parsing is done once at the API
/// boundary and passed through.
#[derive(Debug, Clone)]
pub struct ScoringContext<'a> {
    /// Parsed query containing raw text, constraints, fuzzy parts, and location
    pub query: &'a FFFQuery<'a>,
    pub project_path: Option<&'a Path>,
    pub current_file: Option<&'a str>,
    pub max_typos: u16,
    pub max_threads: usize,
    pub last_same_query_match: Option<QueryMatchEntry>,
    pub combo_boost_score_multiplier: i32,
    pub min_combo_count: u32,
    pub pagination: PaginationArgs,
}

impl ScoringContext<'_> {
    /// Get the effective fuzzy query string for matching.
    /// Returns the first fuzzy part, or the raw query if no parsing was done.
    pub fn effective_query(&self) -> &str {
        match &self.query.fuzzy_query {
            FuzzyQuery::Text(t) => t,
            FuzzyQuery::Parts(parts) if !parts.is_empty() => parts[0],
            _ => self.query.raw_query.trim(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchResult<'a> {
    pub items: Vec<&'a FileItem>,
    pub scores: Vec<Score>,
    pub total_matched: usize,
    pub total_files: usize,
    pub location: Option<Location>,
}

const MAX_MMAP_FILE_SIZE: u64 = 10 * 1024 * 1024;

// Limits the total number of files (and bytes) whose content is kept in
// memory via the `OnceLock<FileContent>` cache. On Unix every cached file
// holds a live `mmap`, which consumes a kernel `vm_map_entry`. On a 500k-file
// monorepo, caching everything exhausts macOS/Linux kernel resources and
// crashes the machine (see issue #294).
//
// Each `FilePicker` owns its own `ContentCacheBudget`. The budget is passed
// to `grep_search` and `warmup_mmaps` so that multiple pickers can coexist
// without interfering with each other's counters.

const MAX_CACHED_CONTENT_BYTES: u64 = 512 * 1024 * 1024;

/// Per-picker budget controlling how many files may have their content
/// persistently cached (mmap on Unix, heap buffer on Windows).
#[derive(Debug)]
pub struct ContentCacheBudget {
    pub max_files: usize,
    pub max_bytes: u64,
    pub max_file_size: u64,
    pub cached_count: AtomicUsize,
    pub cached_bytes: AtomicU64,
}

impl ContentCacheBudget {
    /// No limits — every eligible file is cached. Useful for tests and
    /// short-lived tools that don't need resource protection.
    pub fn unlimited() -> Self {
        Self {
            max_files: usize::MAX,
            max_bytes: u64::MAX,
            max_file_size: MAX_MMAP_FILE_SIZE,
            cached_count: AtomicUsize::new(0),
            cached_bytes: AtomicU64::new(0),
        }
    }

    pub fn zero() -> Self {
        Self {
            max_files: 0,
            max_bytes: 0,
            max_file_size: 0,
            cached_count: AtomicUsize::new(0),
            cached_bytes: AtomicU64::new(0),
        }
    }

    pub fn new_for_repo(file_count: usize) -> Self {
        let max_files = if file_count > 50_000 {
            5_000
        } else if file_count > 10_000 {
            10_000
        } else {
            30_000 // effectively unlimited for small repos
        };

        let max_bytes = if file_count > 50_000 {
            128 * 1024 * 1024 // 128 MB
        } else if file_count > 10_000 {
            256 * 1024 * 1024 // 256 MB
        } else {
            MAX_CACHED_CONTENT_BYTES // 512 MB
        };

        Self {
            max_files,
            max_bytes,
            max_file_size: MAX_MMAP_FILE_SIZE,
            cached_count: AtomicUsize::new(0),
            cached_bytes: AtomicU64::new(0),
        }
    }

    /// Reset the counters. Called when the file index is rebuilt (rescan /
    /// directory change) and all old `FileItem`s are dropped.
    pub fn reset(&self) {
        self.cached_count.store(0, Ordering::Relaxed);
        self.cached_bytes.store(0, Ordering::Relaxed);
    }
}

impl Default for ContentCacheBudget {
    fn default() -> Self {
        Self::new_for_repo(30_000)
    }
}

/// Temporary dense builder for the bigram index.
/// Uses AtomicU64 for lock-free concurrent writes during the parallel build phase.
/// Columns are allocated lazily on first use to avoid the massive upfront allocation
/// (previously ~300MB for 500k files, now proportional to actual bigrams found).
/// Call `compress()` to produce the final compact `BigramIndex`.
pub struct BigramIndexBuilder {
    lookup: Vec<AtomicU32>,
    /// Per-column bitset data, lazily allocated via OnceLock.
    col_data: Vec<OnceLock<Box<[AtomicU64]>>>,
    next_column: AtomicU32,
    words: usize,
    file_count: usize,
    populated: AtomicUsize,
}

impl BigramIndexBuilder {
    pub fn new(file_count: usize) -> Self {
        let words = file_count.div_ceil(64);
        let mut lookup = Vec::with_capacity(65536);
        lookup.resize_with(65536, || AtomicU32::new(NO_COLUMN));
        let mut col_data = Vec::with_capacity(MAX_BIGRAM_COLUMNS);
        col_data.resize_with(MAX_BIGRAM_COLUMNS, OnceLock::new);
        Self {
            lookup,
            col_data,
            next_column: AtomicU32::new(0),
            words,
            file_count,
            populated: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn get_or_alloc_column(&self, key: u16) -> u32 {
        let current = self.lookup[key as usize].load(Ordering::Relaxed);
        if current != NO_COLUMN {
            return current;
        }
        let new_col = self.next_column.fetch_add(1, Ordering::Relaxed);
        if new_col >= MAX_BIGRAM_COLUMNS as u32 {
            return NO_COLUMN;
        }

        match self.lookup[key as usize].compare_exchange(
            NO_COLUMN,
            new_col,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => new_col,
            Err(existing) => existing,
        }
    }

    /// Get (or lazily allocate) the bitset for a given column index.
    #[inline]
    fn column_bitset(&self, col: u32) -> &[AtomicU64] {
        let words = self.words;
        self.col_data[col as usize].get_or_init(|| {
            let mut v = Vec::with_capacity(words);
            v.resize_with(words, || AtomicU64::new(0));
            v.into_boxed_slice()
        })
    }

    pub fn add_file_content(&self, file_idx: usize, content: &[u8]) {
        if content.len() < 2 {
            return;
        }

        debug_assert!(file_idx < self.file_count);
        let word_idx = file_idx / 64;
        let bit_mask = 1u64 << (file_idx % 64);

        let mut prev = content[0];
        for &b in &content[1..] {
            if (32..=126).contains(&prev) && (32..=126).contains(&b) {
                let key = (prev.to_ascii_lowercase() as u16) << 8 | b.to_ascii_lowercase() as u16;
                let col = self.get_or_alloc_column(key);
                if col != NO_COLUMN {
                    self.column_bitset(col)[word_idx].fetch_or(bit_mask, Ordering::Relaxed);
                }
            }
            prev = b;
        }
        self.populated.fetch_add(1, Ordering::Relaxed);
    }

    /// Index skip-1 bigrams (stride 2) for a single file.
    ///
    /// For content "ABCDE" this extracts pairs (A,C), (B,D), (C,E).
    /// These capture non-adjacent character relationships that are largely
    /// independent from consecutive bigrams, enabling much tighter candidate
    /// filtering when ANDead together.
    pub fn add_file_content_skip(&self, file_idx: usize, content: &[u8]) {
        if content.len() < 3 {
            return;
        }

        debug_assert!(file_idx < self.file_count);
        let word_idx = file_idx / 64;
        let bit_mask = 1u64 << (file_idx % 64);

        for i in 0..content.len() - 2 {
            let a = content[i];
            let b = content[i + 2];
            if (32..=126).contains(&a) && (32..=126).contains(&b) {
                let key = (a.to_ascii_lowercase() as u16) << 8 | b.to_ascii_lowercase() as u16;
                let col = self.get_or_alloc_column(key);
                if col != NO_COLUMN {
                    self.column_bitset(col)[word_idx].fetch_or(bit_mask, Ordering::Relaxed);
                }
            }
        }
        self.populated.fetch_add(1, Ordering::Relaxed);
    }

    pub fn is_ready(&self) -> bool {
        self.populated.load(Ordering::Relaxed) > 0
    }

    pub fn columns_used(&self) -> u32 {
        self.next_column
            .load(Ordering::Relaxed)
            .min(MAX_BIGRAM_COLUMNS as u32)
    }

    /// Compress the dense builder into a compact `BigramFilter`.
    ///
    /// Retains columns where the bigram appears in ≥`min_density_pct`% (or
    /// the default ~3.1% heuristic when `None`) and <90% of indexed files.
    /// Sparse columns carry too little data to justify their memory;
    /// ubiquitous columns (≥90%) are nearly all-ones and barely filter.
    ///
    /// Each column's `Box<[AtomicU64]>` (~60 KB for 500k files) is freed
    /// immediately after compression via `OnceLock::take`, so peak memory
    /// during compress is roughly `max(builder, result)` instead of
    /// `builder + result`.
    pub fn compress(self, min_density_pct: Option<u32>) -> BigramFilter {
        let cols = self.columns_used() as usize;
        let words = self.words;
        let file_count = self.file_count;
        let populated = self.populated.load(Ordering::Relaxed);
        let dense_bytes = words * 8; // cost of one dense column

        // Destructure so we can incrementally free col_data entries.
        let old_lookup = self.lookup;
        let mut col_data = self.col_data;

        let mut lookup = vec![NO_COLUMN; 65536];
        let mut dense_data: Vec<u64> = Vec::with_capacity(cols * words);
        let mut dense_count: usize = 0;

        for key in 0..65536u32 {
            let old_col = old_lookup[key as usize].load(Ordering::Relaxed);
            if old_col == NO_COLUMN || old_col as usize >= cols {
                continue;
            }
            let Some(bitset) = col_data[old_col as usize].take() else {
                continue;
            };

            // Count set bits to decide if this column is worth keeping.
            let mut popcount = 0u32;
            for w in 0..words {
                popcount += bitset[w].load(Ordering::Relaxed).count_ones();
            }

            // Sparse threshold — drop bigrams appearing in too few files.
            let sparse_ok = if let Some(min_pct) = min_density_pct {
                // Percentage-based: require ≥ min_pct% of populated files.
                populated > 0 && (popcount as usize) * 100 >= populated * min_pct as usize
            } else {
                // Default heuristic: popcount ≥ words × 2 (~3.1% of files).
                (popcount as usize * 4) >= dense_bytes
            };
            if !sparse_ok {
                continue;
            }

            // Drop ubiquitous bigrams — columns ≥90% ones carry almost no
            // filtering power and just waste memory + AND cycles.
            if populated > 0 && (popcount as usize) * 10 >= populated * 9 {
                continue;
            }

            let dense_idx = dense_count as u32;
            lookup[key as usize] = dense_idx;
            dense_count += 1;

            for w in 0..words {
                dense_data.push(bitset[w].load(Ordering::Relaxed));
            }
        }

        drop(col_data);
        drop(old_lookup);

        BigramFilter {
            lookup,
            dense_data,
            dense_count,
            words,
            file_count,
            populated,
            skip_index: None,
        }
    }
}

unsafe impl Send for BigramIndexBuilder {}
unsafe impl Sync for BigramIndexBuilder {}

/// Compressed bigram inverted index (dense-only).
///
/// Built from `BigramIndexBuilder::compress()`. All columns are dense bitsets
/// packed contiguously in `dense_data` at a fixed stride of `words` — column
/// `i` lives at `i * words`. The `lookup` table maps bigram key → column
/// index directly, so the query path is: one lookup load → one multiply →
/// data access (no pointer chase, no enum discriminant check, SIMD-vectorized
/// AND).
#[derive(Debug)]
pub struct BigramFilter {
    lookup: Vec<u32>,
    /// Flat buffer of all dense column data laid out at fixed stride `words`.
    /// Column `i` starts at `i * words`.
    dense_data: Vec<u64>,
    dense_count: usize,
    words: usize,
    file_count: usize,
    populated: usize,
    /// Optional skip-1 bigram index (stride 2). Built from character pairs
    /// at distance 2, e.g. "ABCDE" → (A,C),(B,D),(C,E). ANDead with the
    /// consecutive bigram candidates during query to dramatically reduce
    /// false positives.
    skip_index: Option<Box<BigramFilter>>,
}

/// SIMD-friendly bitwise AND of two equal-length bitsets.
// Auto vectorized (don't touch)
#[inline]
fn bitset_and(result: &mut [u64], bitset: &[u64]) {
    result
        .iter_mut()
        .zip(bitset.iter())
        .for_each(|(r, b)| *r &= *b);
}

impl BigramFilter {
    /// AND the posting lists for all query bigrams (consecutive + skip).
    /// Returns None if no query bigrams are tracked.
    pub fn query(&self, pattern: &[u8]) -> Option<Vec<u64>> {
        if pattern.len() < 2 {
            return None;
        }

        let mut result = vec![u64::MAX; self.words];
        if !self.file_count.is_multiple_of(64) {
            let last = self.words - 1;
            result[last] = (1u64 << (self.file_count % 64)) - 1;
        }

        let words = self.words;
        let mut has_filter = false;

        // ── Consecutive bigrams (stride 1) ─────────────────────────────
        let mut prev = pattern[0];
        for &b in &pattern[1..] {
            if (32..=126).contains(&prev) && (32..=126).contains(&b) {
                let key = (prev.to_ascii_lowercase() as u16) << 8 | b.to_ascii_lowercase() as u16;
                let col = self.lookup[key as usize];
                if col != NO_COLUMN {
                    let offset = col as usize * words;
                    // SAFETY: compress() guarantees offset + words <= dense_data.len()
                    let slice = unsafe { self.dense_data.get_unchecked(offset..offset + words) };
                    bitset_and(&mut result, slice);
                    has_filter = true;
                }
            }
            prev = b;
        }

        // ── Skip-1 bigrams (stride 2) ──────────────────────────────────
        if let Some(skip) = &self.skip_index
            && pattern.len() >= 3
            && let Some(skip_candidates) = skip.query_skip(pattern)
        {
            bitset_and(&mut result, &skip_candidates);
            has_filter = true;
        }

        has_filter.then_some(result)
    }

    /// Query using stride-2 bigrams from the pattern.
    /// For "ABCDE" queries with keys (A,C), (B,D), (C,E).
    fn query_skip(&self, pattern: &[u8]) -> Option<Vec<u64>> {
        let mut result = vec![u64::MAX; self.words];
        if !self.file_count.is_multiple_of(64) {
            let last = self.words - 1;
            result[last] = (1u64 << (self.file_count % 64)) - 1;
        }

        let words = self.words;
        let mut has_filter = false;

        for i in 0..pattern.len().saturating_sub(2) {
            let a = pattern[i];
            let b = pattern[i + 2];
            if (32..=126).contains(&a) && (32..=126).contains(&b) {
                let key = (a.to_ascii_lowercase() as u16) << 8 | b.to_ascii_lowercase() as u16;
                let col = self.lookup[key as usize];
                if col != NO_COLUMN {
                    let offset = col as usize * words;
                    let slice = unsafe { self.dense_data.get_unchecked(offset..offset + words) };
                    bitset_and(&mut result, slice);
                    has_filter = true;
                }
            }
        }

        has_filter.then_some(result)
    }

    /// Attach a skip-1 bigram index for tighter candidate filtering.
    pub fn set_skip_index(&mut self, skip: BigramFilter) {
        self.skip_index = Some(Box::new(skip));
    }

    #[inline]
    pub fn is_candidate(candidates: &[u64], file_idx: usize) -> bool {
        let word = file_idx / 64;
        let bit = file_idx % 64;
        word < candidates.len() && candidates[word] & (1u64 << bit) != 0
    }

    pub fn count_candidates(candidates: &[u64]) -> usize {
        candidates.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn is_ready(&self) -> bool {
        self.populated > 0
    }

    pub fn file_count(&self) -> usize {
        self.file_count
    }

    pub fn columns_used(&self) -> usize {
        self.dense_count
    }

    /// Total heap bytes used by this index (lookup + dense data + skip).
    pub fn heap_bytes(&self) -> usize {
        let lookup_bytes = self.lookup.len() * std::mem::size_of::<u32>();
        let dense_bytes = self.dense_data.len() * std::mem::size_of::<u64>();
        let skip_bytes = self.skip_index.as_ref().map_or(0, |s| s.heap_bytes());
        lookup_bytes + dense_bytes + skip_bytes
    }

    /// Check whether a bigram key is present in this index.
    pub fn has_key(&self, key: u16) -> bool {
        self.lookup[key as usize] != NO_COLUMN
    }
}

/// Extract deduplicated bigram keys from file content.
/// Same logic as `BigramIndexBuilder::add_file_content`: consecutive printable
/// ASCII pairs, lowercased, encoded as `(prev << 8) | cur`.
pub fn extract_bigrams(content: &[u8]) -> Vec<u16> {
    if content.len() < 2 {
        return Vec::new();
    }
    // Use a flat bitset (65536 bits = 8 KB) for dedup — faster than HashSet.
    let mut seen = vec![0u64; 1024]; // 1024 * 64 = 65536 bits
    let mut bigrams = Vec::new();

    let mut prev = content[0];
    for &b in &content[1..] {
        if (32..=126).contains(&prev) && (32..=126).contains(&b) {
            let key = (prev.to_ascii_lowercase() as u16) << 8 | b.to_ascii_lowercase() as u16;
            let word = key as usize / 64;
            let bit = 1u64 << (key as usize % 64);
            if seen[word] & bit == 0 {
                seen[word] |= bit;
                bigrams.push(key);
            }
        }
        prev = b;
    }
    bigrams
}

/// Tracks bigram changes since the base `BigramFilter` was built.
///
/// Modified and added files store their own bigram sets. Deleted files are
/// tombstoned in a bitset so they can be excluded from base query results.
/// This overlay is updated by the background watcher on every file event
/// and cleared when the base index is rebuilt.
#[derive(Debug)]
pub struct BigramOverlay {
    /// Per-file bigram sets for files modified since the base was built.
    /// Key = file index in the base `Vec<FileItem>`.
    modified: AHashMap<usize, Vec<u16>>,

    /// Tombstone bitset — one bit per base file. Set bits are excluded
    /// from base query results.
    tombstones: Vec<u64>,

    /// Bigram sets for files added after the base was built (overflow files).
    added: Vec<Vec<u16>>,

    /// Number of base files this overlay was created for.
    base_file_count: usize,
}

impl BigramOverlay {
    pub fn new(base_file_count: usize) -> Self {
        let words = base_file_count.div_ceil(64);
        Self {
            modified: AHashMap::new(),
            tombstones: vec![0u64; words],
            added: Vec::new(),
            base_file_count,
        }
    }

    /// Record updated bigram data for a modified base file.
    pub fn modify_file(&mut self, file_idx: usize, content: &[u8]) {
        self.modified.insert(file_idx, extract_bigrams(content));
    }

    /// Tombstone a deleted base file.
    pub fn delete_file(&mut self, file_idx: usize) {
        if file_idx < self.base_file_count {
            let word = file_idx / 64;
            self.tombstones[word] |= 1u64 << (file_idx % 64);
        }
        self.modified.remove(&file_idx);
    }

    /// Record bigrams for a newly added (overflow) file.
    pub fn add_file(&mut self, content: &[u8]) {
        self.added.push(extract_bigrams(content));
    }

    /// Return base file indices of modified files whose bigrams match ALL
    /// of the given `pattern_bigrams`.
    pub fn query_modified(&self, pattern_bigrams: &[u16]) -> Vec<usize> {
        if pattern_bigrams.is_empty() {
            return self.modified.keys().copied().collect();
        }
        self.modified
            .iter()
            .filter_map(|(&file_idx, bigrams)| {
                pattern_bigrams
                    .iter()
                    .all(|pb| bigrams.contains(pb))
                    .then_some(file_idx)
            })
            .collect()
    }

    /// Return overflow indices (into the `added` vec) whose bigrams match
    /// ALL of the given `pattern_bigrams`.
    pub fn query_added(&self, pattern_bigrams: &[u16]) -> Vec<usize> {
        if pattern_bigrams.is_empty() {
            return (0..self.added.len()).collect();
        }
        self.added
            .iter()
            .enumerate()
            .filter_map(|(idx, bigrams)| {
                pattern_bigrams
                    .iter()
                    .all(|pb| bigrams.contains(pb))
                    .then_some(idx)
            })
            .collect()
    }

    /// Get the tombstone bitset for clearing base candidates.
    pub fn tombstones(&self) -> &[u64] {
        &self.tombstones
    }

    pub fn is_tombstoned(&self, file_idx: usize) -> bool {
        let word = file_idx / 64;
        word < self.tombstones.len() && self.tombstones[word] & (1u64 << (file_idx % 64)) != 0
    }

    pub fn base_file_count(&self) -> usize {
        self.base_file_count
    }

    /// Remove an overflow entry by index (when the file is deleted).
    pub fn remove_added(&mut self, idx: usize) {
        if idx < self.added.len() {
            self.added.remove(idx);
        }
    }

    /// Update an existing overflow entry's bigrams.
    pub fn update_added(&mut self, idx: usize, bigrams: Vec<u16>) {
        if idx < self.added.len() {
            self.added[idx] = bigrams;
        }
    }

    /// Total number of entries tracked (for deciding when to trigger a full rebuild).
    pub fn overlay_size(&self) -> usize {
        self.modified.len()
            + self.added.len()
            + self
                .tombstones
                .iter()
                .map(|w| w.count_ones() as usize)
                .sum::<usize>()
    }
}
