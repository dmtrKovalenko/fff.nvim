//! Thread-safe lazy memory-map cache for file contents.
//!
//! Files are mapped on first grep access and cached until invalidated by the
//! background file watcher. Uses `parking_lot::RwLock` for minimal contention
//! on the hot read path during parallel grep.

use memmap2::Mmap;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Thread-safe lazy mmap cache. Files are mapped on first access and held
/// until explicitly invalidated (file change) or cleared (full rescan).
pub struct MmapCache {
    cache: RwLock<HashMap<PathBuf, Arc<Mmap>>>,
    max_file_size: u64,
}

impl MmapCache {
    pub fn new(max_file_size: u64) -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            max_file_size,
        }
    }

    /// Get a cached mmap or create a new one. Returns `None` if the file
    /// is too large, empty, or can't be opened/mapped.
    ///
    /// The returned `Arc<Mmap>` can be held across lock boundaries safely —
    /// even if the entry is evicted from the cache, the mmap stays alive
    /// until all Arc references are dropped.
    #[inline]
    pub fn get_or_insert(&self, path: &Path, size: u64) -> Option<Arc<Mmap>> {
        // Fast path: read lock only
        {
            let cache = self.cache.read();
            if let Some(mmap) = cache.get(path) {
                return Some(Arc::clone(mmap));
            }
        }

        // Slow path: size check + mmap creation + write lock
        if size == 0 || size > self.max_file_size {
            return None;
        }

        let file = File::open(path).ok()?;
        // SAFETY: We invalidate the cache entry when the background watcher
        // detects file modifications. Concurrent readers hold Arc<Mmap> which
        // remains valid even after eviction. The only risk is SIGBUS if the
        // file is truncated while mapped — this is acceptable for a code search
        // tool since source files are rarely truncated in-place.
        let mmap = unsafe { Mmap::map(&file) }.ok()?;
        let arc = Arc::new(mmap);

        let mut cache = self.cache.write();
        // Double-check: another thread may have inserted while we were mapping
        cache
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::clone(&arc));
        Some(arc)
    }

    /// Get a cached mmap without creating one. Used when we want to check
    /// if a file is already cached without triggering I/O.
    #[allow(dead_code)]
    #[inline]
    pub fn get(&self, path: &Path) -> Option<Arc<Mmap>> {
        self.cache.read().get(path).map(Arc::clone)
    }

    /// Remove a single entry. Called by background watcher on file change.
    #[inline]
    pub fn invalidate(&self, path: &Path) {
        self.cache.write().remove(path);
    }

    /// Clear all entries. Called on full rescan.
    pub fn clear(&self) {
        self.cache.write().clear();
    }

    /// Number of cached entries (for diagnostics).
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.cache.read().len()
    }
}
