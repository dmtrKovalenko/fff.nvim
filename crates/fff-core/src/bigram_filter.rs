use std::sync::atomic::{AtomicU16, AtomicU64, AtomicUsize, Ordering};

use ahash::AHashMap;

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
    /// Per-column bitset data, lazily allocated via OnceLock.
    col_data: Vec<AtomicU64>,
    next_column: AtomicU16,
    words: usize,
    file_count: usize,
    populated: AtomicUsize,
}

impl BigramIndexBuilder {
    pub fn new(file_count: usize) -> Self {
        let words = file_count.div_ceil(64);
        let mut lookup = Vec::with_capacity(65536);
        lookup.resize_with(65536, || AtomicU16::new(NO_COLUMN));
        let mut col_data = Vec::with_capacity(MAX_BIGRAM_COLUMNS * words);
        col_data.resize_with(MAX_BIGRAM_COLUMNS * words, || AtomicU64::new(0));
        Self {
            lookup,
            col_data,
            next_column: AtomicU16::new(0),
            words,
            file_count,
            populated: AtomicUsize::new(0),
        }
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

    #[inline]
    fn column_bitset(&self, col: u16) -> &[AtomicU64] {
        let start = col as usize * self.words;
        &self.col_data[start..start + self.words]
    }

    pub(crate) fn add_file_content(&self, skip_builder: &Self, file_idx: usize, content: &[u8]) {
        if content.len() < 2 {
            return;
        }

        debug_assert!(file_idx < self.file_count);
        let word_idx = file_idx / 64;
        let bit_mask = 1u64 << (file_idx % 64);

        // Stack-local dedup bitsets: 1024 × u64 = 8 KB each, covers all 65536 bigrams with margin
        // have to fit in L1 cache
        let mut seen_consec = [0u64; 1024];
        let mut seen_skip = [0u64; 1024];

        let bytes = content;
        let len = bytes.len();

        // First consecutive pair (no skip bigram possible yet).
        let (a, b) = (bytes[0], bytes[1]);
        if (32..=126).contains(&a) && (32..=126).contains(&b) {
            let key = (a.to_ascii_lowercase() as u16) << 8 | b.to_ascii_lowercase() as u16;
            let w = key as usize >> 6;
            let bit = 1u64 << (key as usize & 63);
            seen_consec[w] |= bit;
            let col = self.get_or_alloc_column(key);
            if col != NO_COLUMN {
                self.column_bitset(col)[word_idx].fetch_or(bit_mask, Ordering::Relaxed);
            }
        }

        // Main loop: consecutive (i-1, i) and skip-1 (i-2, i)
        for i in 2..len {
            let cur = bytes[i];

            // Consecutive bigram: (bytes[i-1], bytes[i])
            let prev = bytes[i - 1];
            if (32..=126).contains(&prev) && (32..=126).contains(&cur) {
                let key = (prev.to_ascii_lowercase() as u16) << 8 | cur.to_ascii_lowercase() as u16;
                let w = key as usize >> 6;
                let bit = 1u64 << (key as usize & 63);
                if seen_consec[w] & bit == 0 {
                    seen_consec[w] |= bit;
                    let col = self.get_or_alloc_column(key);
                    if col != NO_COLUMN {
                        self.column_bitset(col)[word_idx].fetch_or(bit_mask, Ordering::Relaxed);
                    }
                }
            }

            // Skip-1 bigram: (bytes[i-2], bytes[i])
            let skip_prev = bytes[i - 2];
            if (32..=126).contains(&skip_prev) && (32..=126).contains(&cur) {
                let key =
                    (skip_prev.to_ascii_lowercase() as u16) << 8 | cur.to_ascii_lowercase() as u16;
                let w = key as usize >> 6;
                let bit = 1u64 << (key as usize & 63);
                if seen_skip[w] & bit == 0 {
                    seen_skip[w] |= bit;
                    let col = skip_builder.get_or_alloc_column(key);
                    if col != NO_COLUMN {
                        skip_builder.column_bitset(col)[word_idx]
                            .fetch_or(bit_mask, Ordering::Relaxed);
                    }
                }
            }
        }

        self.populated.fetch_add(1, Ordering::Relaxed);
        skip_builder.populated.fetch_add(1, Ordering::Relaxed);
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
    pub fn compress(self, min_density_pct: Option<u32>) -> BigramFilter {
        let cols = self.columns_used() as usize;
        let words = self.words;
        let file_count = self.file_count;
        let populated = self.populated.load(Ordering::Relaxed);
        let dense_bytes = words * 8; // cost of one dense column

        let old_lookup = self.lookup;
        let col_data = self.col_data;

        let mut lookup: Vec<u16> = vec![NO_COLUMN; 65536];
        let mut dense_data: Vec<u64> = Vec::with_capacity(cols * words);
        let mut dense_count: usize = 0;

        for key in 0..65536usize {
            let old_col = old_lookup[key].load(Ordering::Relaxed);
            if old_col == NO_COLUMN || old_col as usize >= cols {
                continue;
            }

            let col_start = old_col as usize * words;
            let bitset = &col_data[col_start..col_start + words];

            // count set bits to decide if this column is worth keeping.
            let mut popcount = 0u32;
            for column in bitset.iter().take(words) {
                popcount += column.load(Ordering::Relaxed).count_ones();
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

            for column in bitset.iter().take(words) {
                dense_data.push(column.load(Ordering::Relaxed));
            }
        }

        // col_data + old_lookup dropped here — single deallocation each,
        // no fragmentation.

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
}
