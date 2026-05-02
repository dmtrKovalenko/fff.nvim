use ahash::AHashMap;
use rayon::iter::{IndexedParallelIterator, ParallelIterator};
use rayon::slice::ParallelSlice;
use std::cell::UnsafeCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};

/// Maximum number of distinct bigrams tracked in the inverted index.
/// 95 printable ASCII chars (32..=126) after lowercasing → ~70 distinct → 4900 possible.
/// We cap at 5000 to cover all printable bigrams with margin.
/// 5000 columns × 62.5KB (500k files) = 305MB. For 50k files: 30MB.
const MAX_BIGRAM_COLUMNS: usize = 5000;

/// Sentinel value: bigram has no allocated column.
const NO_COLUMN: u16 = u16::MAX;

/// Temporary sync dense builder for the bigram index.
/// Builds from the many threads reading file contents in parallel
pub struct BigramIndexBuilder {
    // we use lookup as atomics only in the builder because it is filled by the rayon threads
    // the actual index uses pure u16 for the allocations
    lookup: Vec<AtomicU16>,
    /// Flat bitset data, materialised on first use.
    col_data: OnceLock<UnsafeCell<Box<[u64]>>>,
    next_column: AtomicU16,
    words: usize,
    file_count: usize,
    populated: AtomicUsize,
}

// SAFETY: `col_data`'s interior mutability is coordinated via disjoint
// `word_idx` ranges (word-aligned file partitioning in the driver), so
// concurrent access is safe despite the `UnsafeCell`. See builder doc.
unsafe impl Sync for BigramIndexBuilder {}

impl BigramIndexBuilder {
    pub fn new(file_count: usize) -> Self {
        let words = file_count.div_ceil(64);
        let mut lookup = Vec::with_capacity(65536);
        lookup.resize_with(65536, || AtomicU16::new(NO_COLUMN));
        Self {
            lookup,
            col_data: OnceLock::new(),
            next_column: AtomicU16::new(0),
            words,
            file_count,
            populated: AtomicUsize::new(0),
        }
    }

    /// Lazily materialise the full `MAX_BIGRAM_COLUMNS * words` bitset
    /// on first access.
    #[inline(always)]
    fn col_data_cell(&self) -> &UnsafeCell<Box<[u64]>> {
        self.col_data.get_or_init(|| {
            let total = MAX_BIGRAM_COLUMNS * self.words;
            UnsafeCell::new(vec![0u64; total].into_boxed_slice())
        })
    }

    /// Raw pointer to the start of the bitset slab. Used for in-place
    /// `|=` writes under the partitioning invariant.
    #[inline(always)]
    fn col_data_ptr(&self) -> *mut u64 {
        unsafe { (*self.col_data_cell().get()).as_mut_ptr() }
    }

    #[inline]
    fn get_or_alloc_column(&self, key: u16) -> u16 {
        let current = self.lookup[key as usize].load(Ordering::Relaxed);
        if current != NO_COLUMN {
            return current;
        }
        let new_col = self.next_column.fetch_add(1, Ordering::Relaxed);
        if new_col >= MAX_BIGRAM_COLUMNS as u16 {
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

    /// SAFETY: caller must not access the same `word_idx` slot from
    /// another thread concurrently. Partitioning in
    /// `file_picker::build_bigram_index` enforces this.
    #[inline(always)]
    unsafe fn column_word_ptr(&self, col: u16, word_idx: usize) -> *mut u64 {
        unsafe {
            self.col_data_ptr()
                .add(col as usize * self.words + word_idx)
        }
    }

    /// Test/bench accessor for a column's raw bitset words. Assumes the
    /// caller has joined all writers (no concurrent mutation).
    #[cfg(test)]
    fn column_bitset(&self, col: u16) -> &[u64] {
        let start = col as usize * self.words;
        let slab = unsafe { &*self.col_data_cell().get() };
        &slab[start..start + self.words]
    }

    // `pub` (via `#[doc(hidden)]`) only so the criterion bench can drive
    // `add_file_content` directly. External consumers should use
    // `build_bigram_index` instead.
    ///
    /// SAFETY: concurrent callers must partition `file_idx` by
    /// word-aligned ranges so that `file_idx / 64` never collides across
    /// threads. The `file_picker::build_bigram_index` driver enforces
    /// this via `par_chunks` with a word-aligned chunk size.
    #[doc(hidden)]
    pub fn add_file_content(&self, skip_builder: &Self, file_idx: usize, content: &[u8]) {
        if content.len() < 2 {
            return;
        }

        debug_assert!(file_idx < self.file_count);
        let word_idx = file_idx / 64;
        let bit_mask = 1u64 << (file_idx % 64);

        // Stack-local dedup bitsets: 1024 × u64 = 8 KB each, covers all 65536
        // bigram keys with margin. Has to fit in L1 cache.
        let mut seen_consec = [0u64; 1024];
        let mut seen_skip = [0u64; 1024];

        // Normalise each byte as we stream and carry a 2-byte history
        // across iterations so each input byte is normalised exactly once
        // even though it participates in up to three bigrams (as `cur`,
        // then `prev`, then `skip_prev`). Benchmarked against a NEON
        // pre-pass variant — the pre-pass needs a heap scratch per call,
        // which kills throughput unless content is gigantic. Inline
        // normalisation is the faster choice for realistic file sizes.
        let bytes = content;
        let len = bytes.len();

        let mut n0 = normalize_byte_scalar(bytes[0]);
        let mut n1 = normalize_byte_scalar(bytes[1]);

        if n0 != u16::MAX && n1 != u16::MAX {
            let key = (n0 << 8) | n1;
            self.record_bigram(&mut seen_consec, key, word_idx, bit_mask);
        }

        for &b in &bytes[2..len] {
            let cur = normalize_byte_scalar(b);
            if cur != u16::MAX {
                if n1 != u16::MAX {
                    let key = (n1 << 8) | cur;
                    self.record_bigram(&mut seen_consec, key, word_idx, bit_mask);
                }
                if n0 != u16::MAX {
                    let key = (n0 << 8) | cur;
                    skip_builder.record_bigram(&mut seen_skip, key, word_idx, bit_mask);
                }
            }
            n0 = n1;
            n1 = cur;
        }

        self.populated.fetch_add(1, Ordering::Relaxed);
        skip_builder.populated.fetch_add(1, Ordering::Relaxed);
    }

    /// Mark `key` as present for the file whose column-word is `word_idx`
    /// and bit position is `bit_mask`, de-duplicating via the caller-owned
    /// `seen` bitmap so we only touch the shared column slab at most once
    /// per unique bigram per file.
    ///
    /// SAFETY: under the partitioning invariant on `add_file_content`
    /// the `word_idx` slot this touches is owned exclusively by the
    /// current thread, so a plain `|=` through the raw pointer is
    /// race-free (no atomic RMW needed).
    #[inline(always)]
    fn record_bigram(&self, seen: &mut [u64; 1024], key: u16, word_idx: usize, bit_mask: u64) {
        let k = key as usize;
        let w = k >> 6;
        let bit = 1u64 << (k & 63);
        if seen[w] & bit == 0 {
            seen[w] |= bit;
            let col = self.get_or_alloc_column(key);
            if col != NO_COLUMN {
                unsafe {
                    let p = self.column_word_ptr(col, word_idx);
                    *p |= bit_mask;
                }
            }
        }
    }

    pub fn is_ready(&self) -> bool {
        self.populated.load(Ordering::Relaxed) > 0
    }

    pub fn columns_used(&self) -> u16 {
        self.next_column
            .load(Ordering::Relaxed)
            .min(MAX_BIGRAM_COLUMNS as u16)
    }

    /// Compress the dense builder into a compact `BigramFilter`.
    ///
    /// Retains columns where the bigram appears in ≥`min_density_pct`% (or
    /// the default ~3.1% heuristic when `None`) and <90% of indexed files.
    /// Sparse columns carry too little data to justify their memory;
    /// ubiquitous columns (≥90%) are nearly all-ones and barely filter.
    #[inline(always)]
    pub fn compress(self, min_density_pct: Option<u32>) -> BigramFilter {
        let cols = self.columns_used() as usize;
        let words = self.words;
        let file_count = self.file_count;
        let populated = self.populated.load(Ordering::Relaxed);
        let dense_bytes = words * 8; // cost of one dense column

        let old_lookup = self.lookup;
        // If no file ever populated content, col_data was never
        // materialised. Treat as empty — every column falls through.
        let col_data: Option<Box<[u64]>> = self.col_data.into_inner().map(UnsafeCell::into_inner);

        let mut lookup: Vec<u16> = vec![NO_COLUMN; 65536];
        let mut dense_data: Vec<u64> = Vec::with_capacity(cols * words);
        let mut dense_count: usize = 0;

        if let Some(col_data) = col_data.as_deref() {
            for key in 0..65536usize {
                let old_col = old_lookup[key].load(Ordering::Relaxed);
                if old_col == NO_COLUMN || old_col as usize >= cols {
                    continue;
                }

                let col_start = old_col as usize * words;
                let bitset = &col_data[col_start..col_start + words];

                // count set bits to decide if this column is worth keeping.
                let mut popcount = 0u32;
                for &word in bitset.iter().take(words) {
                    popcount += word.count_ones();
                }

                // drop bigrams appearing in too few files
                let not_to_rare = if let Some(min_pct) = min_density_pct {
                    // Percentage-based: require ≥ min_pct% of populated files.
                    populated > 0 && (popcount as usize) * 100 >= populated * min_pct as usize
                } else {
                    // Default: popcount ≥ words × 2 (~3.1% of files).
                    (popcount as usize * 4) >= dense_bytes
                };

                if !not_to_rare {
                    continue;
                }

                // Drop ubiquitous bigrams — columns ≥90% ones carry almost no
                // filtering power and just waste memory + AND cycles.
                if populated > 0 && (popcount as usize) * 10 >= populated * 9 {
                    continue;
                }

                let dense_idx = dense_count as u16;
                lookup[key] = dense_idx;
                dense_count += 1;

                dense_data.extend_from_slice(bitset);
            }
        }

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

/// Inverted bigram index with optional "skip-1" extension
/// Copmressed into bitset for minimal usage, the layout of this struct actually matters
#[derive(Debug)]
pub struct BigramFilter {
    lookup: Vec<u16>,
    /// Flat buffer of all dense column data laid out at fixed stride `words`.
    /// Column `i` starts at `i * words`.
    dense_data: Vec<u64>, // do not try to change this to u8 it has to be wordsize
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

        // strid-1 bigrams
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
        let lookup_bytes = self.lookup.len() * std::mem::size_of::<u16>();
        let dense_bytes = self.dense_data.len() * std::mem::size_of::<u64>();
        let skip_bytes = self.skip_index.as_ref().map_or(0, |s| s.heap_bytes());
        lookup_bytes + dense_bytes + skip_bytes
    }

    /// Check whether a bigram key is present in this index.
    pub fn has_key(&self, key: u16) -> bool {
        self.lookup[key as usize] != NO_COLUMN
    }

    /// Raw lookup table (65536 entries mapping bigram key → column index).
    pub fn lookup(&self) -> &[u16] {
        &self.lookup
    }

    /// Flat dense bitset data at fixed stride `words`.
    pub fn dense_data(&self) -> &[u64] {
        &self.dense_data
    }

    /// Number of u64 words per column (= ceil(file_count / 64)).
    pub fn words(&self) -> usize {
        self.words
    }

    /// Number of dense columns retained after compression.
    pub fn dense_count(&self) -> usize {
        self.dense_count
    }

    /// Number of files that contributed content to the index.
    pub fn populated(&self) -> usize {
        self.populated
    }

    /// Reference to the optional skip-1 bigram sub-index.
    pub fn skip_index(&self) -> Option<&BigramFilter> {
        self.skip_index.as_deref()
    }

    /// Create a new bigram filter from the internal data
    pub fn new(
        lookup: Vec<u16>,
        dense_data: Vec<u64>,
        dense_count: usize,
        words: usize,
        file_count: usize,
        populated: usize,
    ) -> Self {
        Self {
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

/// Map a single input byte to its normalised form used by the bigram
/// builder: `u16::MAX` when not printable ASCII (outside `32..=126`),
/// otherwise the lowercased byte value in `0..=126`. The `u16::MAX`
/// sentinel can never collide with a printable-ASCII byte so the consumer
/// can test `!= u16::MAX` without false positives.
///
/// Branchless and `#[inline(always)]`: LLVM lifts the ASCII-range check
/// and the conditional-lowercase OR into a handful of instructions per
/// call, so calling this inside a hot loop matches a hand-unrolled
/// equivalent.
#[inline(always)]
fn normalize_byte_scalar(b: u8) -> u16 {
    let printable = b.wrapping_sub(32) <= 94;
    // Branchless lowercase: OR 0x20 iff byte is in 'A'..='Z'.
    let lower = b | ((b.wrapping_sub(b'A') < 26) as u8 * 0x20);
    if printable { lower as u16 } else { u16::MAX }
}

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

    /// Original files count this overlay was created for.
    base_file_count: usize,
}

impl BigramOverlay {
    pub(crate) fn new(base_file_count: usize) -> Self {
        let words = base_file_count.div_ceil(64);
        Self {
            modified: AHashMap::new(),
            tombstones: vec![0u64; words],
            base_file_count,
        }
    }

    pub(crate) fn modify_file(&mut self, file_idx: usize, content: &[u8]) {
        self.modified.insert(file_idx, extract_bigrams(content));
    }

    pub(crate) fn delete_file(&mut self, file_idx: usize) {
        if file_idx < self.base_file_count {
            let word = file_idx / 64;
            self.tombstones[word] |= 1u64 << (file_idx % 64);
        }
        self.modified.remove(&file_idx);
    }

    /// Return base file indices of modified files whose bigrams match ALL
    /// of the given `pattern_bigrams`.
    pub(crate) fn query_modified(&self, pattern_bigrams: &[u16]) -> Vec<usize> {
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

    /// Number of base files this overlay was created for.
    pub(crate) fn base_file_count(&self) -> usize {
        self.base_file_count
    }

    /// Get the tombstone bitset for clearing base candidates.
    pub(crate) fn tombstones(&self) -> &[u64] {
        &self.tombstones
    }

    /// Get all modified file indices (for conservative overlay merging when
    /// we can't extract precise bigrams, e.g. regex patterns).
    pub(crate) fn modified_indices(&self) -> Vec<usize> {
        self.modified.keys().copied().collect()
    }
}

pub const BIGRAM_CONTENT_CAP: usize = 64 * 1024;
const BIGRAM_CHUNK_FILES: usize = 4 * 64;

/// Sparse-column cutoff for the skip-1 sub-index. Rare skip columns add
/// little filtering power but ~25-30% of index memory, so we drop
/// anything appearing in < 12 % of populated files.
const SKIP_INDEX_MIN_DENSITY_PCT: u32 = 12;

thread_local! {
    /// Per-rayon-worker reusable read buffer. 64 KB is too large to
    /// keep on the default pthread stack (macOS ships 512 KB), so the
    /// buffer lives on the heap behind a `Box<[u8; N]>`. TLS keeps the
    /// allocation alive for the thread's lifetime so we pay the cost
    /// once, not per file.
    static READ_BUF: std::cell::RefCell<Box<[u8; BIGRAM_CONTENT_CAP]>> =
        std::cell::RefCell::new(Box::new([0u8; BIGRAM_CONTENT_CAP]));
}

/// Outcome of processing one file's content.
enum FileOutcome {
    /// Content contained a NUL byte — mark the file as binary so future
    /// greps skip it without re-reading.
    Binary,
    /// Read succeeded and the content was fed to the bigram builder.
    Indexed,
    /// File was empty or failed to open; nothing to do.
    Skipped,
}

#[tracing::instrument(skip_all, name = "Building Bigram Index", level = tracing::Level::DEBUG)]
pub(crate) fn build_bigram_index(
    files: &[crate::types::FileItem],
    budget: &crate::types::ContentCacheBudget,
    base_path: &std::path::Path,
    arena: crate::simd_path::ArenaPtr,
) -> (BigramFilter, Vec<usize>) {
    let start = std::time::Instant::now();
    tracing::info!("Building bigram index for {} files...", files.len());

    let builder = BigramIndexBuilder::new(files.len());
    let skip_builder = BigramIndexBuilder::new(files.len());

    // this does remove a memcpy for every single file + actually reducing open time on macos
    #[cfg(unix)]
    let base_fd: libc::c_int = open_base_dir_fd(base_path);
    #[cfg(not(unix))]
    let base_fd: i32 = -1;

    // `content_binary` is only touched from the Binary branch below, so
    // the mutex is cold in practice. A lock-free collector wasn't worth
    // the complexity.
    let content_binary: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

    crate::file_picker::BACKGROUND_THREAD_POOL.install(|| {
        files
            .par_chunks(BIGRAM_CHUNK_FILES)
            .enumerate()
            .for_each(|(chunk_idx, chunk)| {
                let base_idx = chunk_idx * BIGRAM_CHUNK_FILES;
                for (offset, file) in chunk.iter().enumerate() {
                    let file_idx = base_idx + offset;
                    let outcome = process_file(
                        file,
                        file_idx,
                        &builder,
                        &skip_builder,
                        base_fd,
                        base_path,
                        arena,
                        budget,
                    );
                    if matches!(outcome, FileOutcome::Binary) {
                        content_binary.lock().unwrap().push(file_idx);
                    }
                }
            });
    });

    #[cfg(unix)]
    if base_fd >= 0 {
        // SAFETY: we opened `base_fd` at the top of this function and
        // no worker still references it once the rayon pool joined.
        unsafe { libc::close(base_fd) };
    }

    let content_binary_vec = content_binary.into_inner().unwrap();

    let cols = builder.columns_used();
    let mut index = builder.compress(None);
    let skip_index = skip_builder.compress(Some(SKIP_INDEX_MIN_DENSITY_PCT));
    index.set_skip_index(skip_index);

    // Builder buffers were freed by `compress()` above (one deallocation
    // each); nudge mimalloc to return them (and any transient allocs)
    // to the OS.
    crate::file_picker::hint_allocator_collect();

    tracing::info!(
        "Bigram index built in {:.2}s — {} dense columns for {} files",
        start.elapsed().as_secs_f64(),
        cols,
        files.len(),
    );
    if !content_binary_vec.is_empty() {
        tracing::info!(
            "Bigram build detected {} content-binary files (not caught by extension)",
            content_binary_vec.len(),
        );
    }

    (index, content_binary_vec)
}

/// Process one file: read up to `BIGRAM_CONTENT_CAP` bytes, feed them
/// to the bigram builder (or record as binary / skipped).
///
/// `base_fd` is the parent-directory fd for the Unix `openat` fast
/// path, or `-1` to force the portable `std::fs::File::open` fallback.
#[inline]
#[allow(clippy::too_many_arguments)]
fn process_file(
    file: &crate::types::FileItem,
    file_idx: usize,
    builder: &BigramIndexBuilder,
    skip_builder: &BigramIndexBuilder,
    base_fd: i32,
    base_path: &std::path::Path,
    arena: crate::simd_path::ArenaPtr,
    budget: &crate::types::ContentCacheBudget,
) -> FileOutcome {
    if file.is_binary() || file.size == 0 || file.size > budget.max_file_size {
        return FileOutcome::Skipped;
    }

    // Zero-copy fast path: the warmup phase may have cached this file's
    // content already. Avoid re-reading from disk.
    if let Some(cached) = file.get_content(arena, base_path, budget) {
        if crate::file_picker::detect_binary_content(cached) {
            return FileOutcome::Binary;
        }
        let capped = &cached[..cached.len().min(BIGRAM_CONTENT_CAP)];
        builder.add_file_content(skip_builder, file_idx, capped);
        return FileOutcome::Indexed;
    }

    let want = (file.size as usize).min(BIGRAM_CONTENT_CAP);
    let mut path_buf = [0u8; crate::simd_path::PATH_BUF_SIZE];

    READ_BUF.with(|read_cell| {
        let mut buf = read_cell.borrow_mut();
        let filled = read_file_content(
            file,
            base_fd,
            base_path,
            arena,
            &mut path_buf,
            &mut buf[..want],
        );
        if filled == 0 {
            return FileOutcome::Skipped;
        }
        let data = &buf[..filled];
        if crate::file_picker::detect_binary_content(data) {
            return FileOutcome::Binary;
        }
        builder.add_file_content(skip_builder, file_idx, data);
        FileOutcome::Indexed
    })
}

/// Read up to `buf.len()` bytes of `file`'s content into `buf`. Returns
/// the number of bytes actually read (0 on any error, so callers treat
/// failures as "skip").
#[inline]
fn read_file_content(
    file: &crate::types::FileItem,
    base_fd: i32,
    base_path: &std::path::Path,
    arena: crate::simd_path::ArenaPtr,
    path_buf: &mut [u8; crate::simd_path::PATH_BUF_SIZE],
    buf: &mut [u8],
) -> usize {
    #[cfg(unix)]
    {
        read_file_content_unix(file, base_fd, base_path, arena, path_buf, buf)
    }
    #[cfg(not(unix))]
    {
        let _ = base_fd;
        read_file_content_std(file, base_path, arena, path_buf, buf)
    }
}

#[cfg(unix)]
fn read_file_content_unix(
    file: &crate::types::FileItem,
    base_fd: libc::c_int,
    base_path: &std::path::Path,
    arena: crate::simd_path::ArenaPtr,
    path_buf: &mut [u8; crate::simd_path::PATH_BUF_SIZE],
    buf: &mut [u8],
) -> usize {
    let fd = if base_fd >= 0 {
        let rel_cstr = file.write_relative_cstr(arena, path_buf);
        // SAFETY: `rel_cstr` is NUL-terminated, `base_fd` is a valid
        // directory descriptor owned by the caller.
        unsafe { libc::openat(base_fd, rel_cstr.as_ptr(), libc::O_RDONLY) }
    } else {
        use std::os::unix::io::IntoRawFd;
        let abs = file.write_absolute_path(arena, base_path, path_buf);
        match std::fs::File::open(abs) {
            Ok(f) => f.into_raw_fd(),
            Err(_) => return 0,
        }
    };
    if fd < 0 {
        return 0;
    }

    let mut filled = 0usize;
    while filled < buf.len() {
        // SAFETY: `fd` is an owned descriptor, `buf[filled..]` is a
        // valid writable slice for `buf.len() - filled` bytes.
        let n = unsafe {
            libc::read(
                fd,
                buf[filled..].as_mut_ptr() as *mut libc::c_void,
                (buf.len() - filled) as libc::size_t,
            )
        };
        if n <= 0 {
            break;
        }
        filled += n as usize;
    }
    // SAFETY: matching close for the owned descriptor.
    unsafe { libc::close(fd) };
    filled
}

/// Open the base directory for the `openat` fast path. Returns `-1` on
/// failure — callers interpret a negative fd as "fall back to absolute
/// paths".
#[cfg(unix)]
fn open_base_dir_fd(base_path: &std::path::Path) -> libc::c_int {
    use std::os::unix::ffi::OsStrExt;
    let mut cstr = [0u8; crate::simd_path::PATH_BUF_SIZE];
    let bytes = base_path.as_os_str().as_bytes();
    if bytes.len() >= cstr.len() {
        return -1;
    }
    cstr[..bytes.len()].copy_from_slice(bytes);
    // SAFETY: `cstr` is NUL-terminated by construction (zero-initialised,
    // and we only filled up to `bytes.len() < cstr.len()`).
    unsafe {
        libc::open(
            cstr.as_ptr() as *const std::os::raw::c_char,
            libc::O_RDONLY | libc::O_DIRECTORY,
        )
    }
}

/// Portable fallback (Windows + non-`openat` Unix): `std::fs::File` +
/// `Read::read` into `buf`. Used on Windows unconditionally, and on
/// Unix when the base directory fd could not be opened.
#[cfg(not(unix))]
fn read_file_content_std(
    file: &crate::types::FileItem,
    base_path: &std::path::Path,
    arena: crate::simd_path::ArenaPtr,
    path_buf: &mut [u8; crate::simd_path::PATH_BUF_SIZE],
    buf: &mut [u8],
) -> usize {
    use std::io::Read;
    let abs = file.write_absolute_path(arena, base_path, path_buf);
    let Ok(mut f) = std::fs::File::open(abs) else {
        return 0;
    };
    let mut filled = 0usize;
    while filled < buf.len() {
        match f.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return 0,
        }
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a key the same way `add_file_content` does: two printable-ASCII
    /// bytes, lowercased, packed as `(hi << 8) | lo`.
    fn key(a: u8, b: u8) -> u16 {
        ((a.to_ascii_lowercase() as u16) << 8) | b.to_ascii_lowercase() as u16
    }

    /// Return the sorted list of (consec, skip) bigram keys that should appear
    /// for `content`. Used as the reference implementation.
    fn expected_bigrams(content: &[u8]) -> (Vec<u16>, Vec<u16>) {
        let mut consec: std::collections::BTreeSet<u16> = Default::default();
        let mut skip: std::collections::BTreeSet<u16> = Default::default();
        let printable = |b: u8| (32..=126).contains(&b);
        for i in 1..content.len() {
            let a = content[i - 1];
            let b = content[i];
            if printable(a) && printable(b) {
                consec.insert(key(a, b));
            }
            if i >= 2 {
                let a = content[i - 2];
                let b = content[i];
                if printable(a) && printable(b) {
                    skip.insert(key(a, b));
                }
            }
        }
        (consec.into_iter().collect(), skip.into_iter().collect())
    }

    /// Query: does the builder record file 0 as having this bigram set?
    fn builder_has_key_for_file_0(b: &BigramIndexBuilder, k: u16) -> bool {
        let col = b.lookup[k as usize].load(Ordering::Relaxed);
        if col == NO_COLUMN {
            return false;
        }
        b.column_bitset(col)[0] & 1 != 0
    }

    fn run_and_compare(content: &[u8]) {
        let consec = BigramIndexBuilder::new(1);
        let skip = BigramIndexBuilder::new(1);
        consec.add_file_content(&skip, 0, content);

        let (expected_consec, expected_skip) = expected_bigrams(content);

        // Every expected bigram must be recorded.
        for k in &expected_consec {
            assert!(
                builder_has_key_for_file_0(&consec, *k),
                "consec bigram 0x{k:04x} missing for content {content:?}",
            );
        }
        for k in &expected_skip {
            assert!(
                builder_has_key_for_file_0(&skip, *k),
                "skip bigram 0x{k:04x} missing for content {content:?}",
            );
        }

        // No unexpected bigrams — iterate lookup for set columns.
        for k in 0u32..=0xFFFF {
            let recorded_consec = builder_has_key_for_file_0(&consec, k as u16);
            let recorded_skip = builder_has_key_for_file_0(&skip, k as u16);
            if recorded_consec {
                assert!(
                    expected_consec.contains(&(k as u16)),
                    "unexpected consec bigram 0x{k:04x} in content {content:?}",
                );
            }
            if recorded_skip {
                assert!(
                    expected_skip.contains(&(k as u16)),
                    "unexpected skip bigram 0x{k:04x} in content {content:?}",
                );
            }
        }
    }

    #[test]
    fn add_file_empty_is_noop() {
        let consec = BigramIndexBuilder::new(1);
        let skip = BigramIndexBuilder::new(1);
        consec.add_file_content(&skip, 0, b"");
        assert_eq!(consec.columns_used(), 0);
        assert_eq!(skip.columns_used(), 0);
        // populated counter not incremented for empty input
        assert_eq!(consec.populated.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn add_file_single_byte_is_noop() {
        let consec = BigramIndexBuilder::new(1);
        let skip = BigramIndexBuilder::new(1);
        consec.add_file_content(&skip, 0, b"a");
        assert_eq!(consec.columns_used(), 0);
        assert_eq!(skip.columns_used(), 0);
    }

    #[test]
    fn add_file_two_bytes_consec_only() {
        // With exactly 2 bytes there's no skip bigram (needs i >= 2 in the loop).
        run_and_compare(b"ab");
    }

    #[test]
    fn add_file_three_bytes_has_skip() {
        // "abc" -> consec {"ab", "bc"}, skip {"ac"}
        run_and_compare(b"abc");
    }

    #[test]
    fn add_file_ascii_words() {
        run_and_compare(b"hello world");
        run_and_compare(b"the quick brown fox jumps over the lazy dog");
        run_and_compare(b"fn main() { println!(\"hi\"); }");
    }

    #[test]
    fn add_file_case_is_lowered() {
        // Uppercase should be lowercased before keying, so "AB" == "ab".
        let upper = BigramIndexBuilder::new(1);
        let upper_skip = BigramIndexBuilder::new(1);
        upper.add_file_content(&upper_skip, 0, b"ABC");

        let lower = BigramIndexBuilder::new(1);
        let lower_skip = BigramIndexBuilder::new(1);
        lower.add_file_content(&lower_skip, 0, b"abc");

        // Both should have identical bigram keys.
        for k in 0u32..=0xFFFF {
            let u = builder_has_key_for_file_0(&upper, k as u16);
            let l = builder_has_key_for_file_0(&lower, k as u16);
            assert_eq!(u, l, "consec 0x{k:04x}: upper={u} lower={l}");
            let u = builder_has_key_for_file_0(&upper_skip, k as u16);
            let l = builder_has_key_for_file_0(&lower_skip, k as u16);
            assert_eq!(u, l, "skip 0x{k:04x}: upper={u} lower={l}");
        }
    }

    #[test]
    fn add_file_rejects_non_printable() {
        // Bigrams where either byte is outside 32..=126 are rejected. But
        // the skip-1 bigram can still connect two printable bytes across a
        // non-printable one: for "\0a\0b", consec sees no valid pair but
        // skip sees (a,b) at i=3. Use the reference implementation.
        run_and_compare(b"\0a\0b");

        // All-zero input: truly nothing recorded.
        let consec = BigramIndexBuilder::new(1);
        let skip = BigramIndexBuilder::new(1);
        consec.add_file_content(&skip, 0, b"\0\0\0\0");
        assert_eq!(consec.columns_used(), 0);
        assert_eq!(skip.columns_used(), 0);
    }

    #[test]
    fn add_file_mixed_printable_and_control() {
        // "a\tb\nc d" — \t (9) and \n (10) are below 32. Consec:
        //   (a, \t) x, (\t, b) x, (b, \n) x, (\n, c) x, (c, ' ') ok, (' ', d) ok
        // Skip (i-2, i):
        //   (a, b) ok, (\t, \n) x, (b, c) ok, (\n, ' ') x, (c, d) ok
        run_and_compare(b"a\tb\nc d");
    }

    #[test]
    fn add_file_repeats_are_deduped() {
        // "ababab" has many repeats of "ab", "ba" — each unique bigram should
        // be recorded exactly once (the stack-local `seen_*` dedup works).
        run_and_compare(b"ababababab");
    }

    #[test]
    fn add_file_tombstone_separation() {
        // Two separate files share no bits; file 1's content doesn't bleed
        // into file 0's row and vice-versa.
        let consec = BigramIndexBuilder::new(2);
        let skip = BigramIndexBuilder::new(2);
        consec.add_file_content(&skip, 0, b"xy");
        consec.add_file_content(&skip, 1, b"zw");

        let key_xy = key(b'x', b'y');
        let key_zw = key(b'z', b'w');

        // file 0 has "xy" but not "zw"
        let col_xy = consec.lookup[key_xy as usize].load(Ordering::Relaxed);
        let col_zw = consec.lookup[key_zw as usize].load(Ordering::Relaxed);
        let bitset_xy = consec.column_bitset(col_xy)[0];
        let bitset_zw = consec.column_bitset(col_zw)[0];
        assert_eq!(bitset_xy & 0b01, 0b01, "file 0 should have xy");
        assert_eq!(bitset_zw & 0b01, 0, "file 0 should NOT have zw");
        assert_eq!(bitset_xy & 0b10, 0, "file 1 should NOT have xy");
        assert_eq!(bitset_zw & 0b10, 0b10, "file 1 should have zw");
    }

    #[test]
    fn add_file_long_content() {
        // Stress test: ~8 KB of printable ASCII. Should complete without
        // overflowing any stack-local bitset and produce the full set.
        let mut buf = Vec::with_capacity(8192);
        for i in 0..8192 {
            buf.push(32u8 + ((i * 7) % 95) as u8); // cycle through printable range
        }
        run_and_compare(&buf);
    }

    #[test]
    fn add_file_simd_and_scalar_agree() {
        // Cross-check: both code paths (scalar <128 bytes, SIMD ≥128) must
        // produce identical bigram sets for content that straddles the
        // threshold. Mix printable ASCII with some non-printable bytes and
        // repeats so the non-printable branch in the SIMD path exercises.
        let mut mixed = Vec::with_capacity(256);
        for i in 0..256usize {
            mixed.push(match i % 9 {
                0 => 0,     // NUL
                1 => 0x7F,  // DEL (just above 126)
                2 => b'\n', // below 32
                _ => 32 + ((i * 13) % 95) as u8,
            });
        }

        run_and_compare(&mixed[..127]); // scalar path
        run_and_compare(&mixed); // SIMD path (256 bytes)
        run_and_compare(&mixed[..192]); // SIMD path with scalar tail
    }

    #[test]
    fn add_file_respects_file_count_boundary() {
        // file_count=100, file_idx=63 (last bit in word 0) and file_idx=64
        // (first bit in word 1). Make sure the word_idx math is right.
        let consec = BigramIndexBuilder::new(100);
        let skip = BigramIndexBuilder::new(100);
        consec.add_file_content(&skip, 63, b"ab");
        consec.add_file_content(&skip, 64, b"cd");

        let kab = key(b'a', b'b');
        let kcd = key(b'c', b'd');
        let col_ab = consec.lookup[kab as usize].load(Ordering::Relaxed);
        let col_cd = consec.lookup[kcd as usize].load(Ordering::Relaxed);

        let ab_bitset = consec.column_bitset(col_ab);
        let cd_bitset = consec.column_bitset(col_cd);
        // ab in word 0, bit 63
        assert_eq!(ab_bitset[0], 1u64 << 63);
        assert_eq!(ab_bitset[1], 0);
        // cd in word 1, bit 0
        assert_eq!(cd_bitset[0], 0);
        assert_eq!(cd_bitset[1], 1);
    }
}
