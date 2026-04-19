use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};

use crate::constraints::Constrainable;
use crate::query_tracker::QueryMatchEntry;
use crate::simd_path::{ArenaPtr, PATH_BUF_SIZE};
use fff_query_parser::{FFFQuery, FuzzyQuery, Location};

/// Different sources of the string storage used by FFF
/// implements as a deduplicated 16-bytes alined heap
/// can be stored in RAM or on disk
pub trait FFFStringStorage {
    /// Resolve the arena for a [`FileItem`] (handles base vs overflow split).
    fn arena_for(&self, file: &FileItem) -> ArenaPtr;

    /// The base arena (scan-time paths).
    fn base_arena(&self) -> ArenaPtr;
    /// The overflow arena (paths added after the last full scan).
    fn overflow_arena(&self) -> ArenaPtr;
}

impl FFFStringStorage for ArenaPtr {
    #[inline]
    fn arena_for(&self, _file: &FileItem) -> ArenaPtr {
        *self
    }

    #[inline]
    fn base_arena(&self) -> ArenaPtr {
        *self
    }

    #[inline]
    fn overflow_arena(&self) -> ArenaPtr {
        *self
    }
}

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

pub struct FileItemFlags;

impl FileItemFlags {
    pub const BINARY: u8 = 1 << 0;
    /// Tombstone — file was deleted but index slot is preserved so
    /// bigram indices for other files stay valid.
    pub const DELETED: u8 = 1 << 1;
    /// File was added after the last full reindex; its indices point
    /// into the overflow builder arena, not the base arena.
    pub const OVERFLOW: u8 = 1 << 2;
}

pub struct DirFlags;

impl DirFlags {
    pub const OVERFLOW: u8 = 1 << 0;
}

/// A directory in the file index. Shares chunk arena with file paths.
#[derive(Debug)]
pub struct DirItem {
    flags: u8,
    pub(crate) path: crate::simd_path::ChunkedString,
    /// Byte offset where the last path segment begins (e.g. for `src/components/`
    /// this is 4, pointing to `components/`). Used for dirname-bonus scoring.
    last_segment_offset: u16,
    /// Maximum `access_frecency_score` among direct child files.
    /// Atomic so parallel frecency updates can write directly without juggling.
    max_access_frecency: AtomicI32,
}

impl Clone for DirItem {
    fn clone(&self) -> Self {
        Self {
            flags: self.flags,
            path: self.path.clone(),
            last_segment_offset: self.last_segment_offset,
            max_access_frecency: AtomicI32::new(self.max_access_frecency()),
        }
    }
}

impl DirItem {
    #[inline(always)]
    pub fn is_overflow(&self) -> bool {
        self.flags & DirFlags::OVERFLOW == 0
    }

    pub(crate) fn new(path: crate::simd_path::ChunkedString, last_segment_offset: u16) -> Self {
        Self {
            path,
            flags: 0,
            last_segment_offset,
            max_access_frecency: AtomicI32::new(0),
        }
    }

    /// Byte offset of the last path segment within the directory path.
    #[inline]
    pub fn last_segment_offset(&self) -> u16 {
        self.last_segment_offset
    }

    /// Current max access frecency score.
    #[inline]
    pub fn max_access_frecency(&self) -> i32 {
        self.max_access_frecency.load(Ordering::Relaxed)
    }

    /// Atomically update the directory's frecency score if the given score is larger.
    /// Safe to call from parallel threads.
    #[inline]
    pub fn update_frecency_if_larger(&self, score: i32) {
        self.max_access_frecency.fetch_max(score, Ordering::Relaxed);
    }

    /// Reset frecency to zero (used before full recomputation).
    #[inline]
    pub fn reset_frecency(&self) {
        self.max_access_frecency.store(0, Ordering::Relaxed);
    }

    pub(crate) fn read_relative_path<'a>(&self, arena: ArenaPtr, buf: &'a mut [u8]) -> &'a str {
        self.path.read_to_buf(arena, buf)
    }

    /// Relative dir path as owned String (cold path).
    pub fn relative_path(&self, arena: impl FFFStringStorage) -> String {
        let mut out = String::new();
        let ptr = if self.is_overflow() {
            arena.overflow_arena()
        } else {
            arena.base_arena()
        };

        self.path.write_to_string(ptr, &mut out);
        out
    }

    /// Write the last segment (dirname) of this directory path to `out`.
    pub fn write_dir_name(&self, arena: ArenaPtr, out: &mut String) {
        out.clear();
        let total = self.path.byte_len as usize;
        let offset = self.last_segment_offset as usize;
        if offset >= total {
            return;
        }
        // Read the full path, then slice from last_segment_offset
        let mut buf = [0u8; PATH_BUF_SIZE];
        let full = self.path.read_to_buf(arena, &mut buf);
        out.push_str(&full[offset..]);
    }

    /// The dirname (last segment) as an owned String. Cold path.
    pub fn dir_name(&self, arena: impl FFFStringStorage) -> String {
        let mut out = String::new();
        let ptr = if self.is_overflow() {
            arena.overflow_arena()
        } else {
            arena.base_arena()
        };
        self.write_dir_name(ptr, &mut out);
        out
    }

    /// A path = base_path + "/" + relative. Cold path, allocates.
    pub fn absolute_path(&self, arena: impl FFFStringStorage, base_path: &Path) -> PathBuf {
        let rel = self.relative_path(arena);
        if rel.is_empty() {
            base_path.to_path_buf()
        } else {
            base_path.join(&rel)
        }
    }
}

impl Constrainable for DirItem {
    #[inline]
    fn write_file_name(&self, arena: ArenaPtr, out: &mut String) {
        // For dirs, the "file name" equivalent is the last path segment
        self.write_dir_name(arena, out);
    }

    #[inline]
    fn write_relative_path(&self, arena: ArenaPtr, out: &mut String) {
        self.path.write_to_string(arena, out);
    }

    #[inline]
    fn git_status(&self) -> Option<git2::Status> {
        None
    }
}

#[derive(Debug)]
pub struct FileItem {
    pub size: u64,
    pub modified: u64,
    pub access_frecency_score: i16,
    pub modification_frecency_score: i16,
    pub git_status: Option<git2::Status>,
    pub(crate) path: crate::simd_path::ChunkedString,
    parent_dir: u32,
    flags: u8,
    content: OnceLock<FileContent>,
}

impl Clone for FileItem {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            parent_dir: self.parent_dir,
            size: self.size,
            modified: self.modified,
            access_frecency_score: self.access_frecency_score,
            modification_frecency_score: self.modification_frecency_score,
            git_status: self.git_status,
            flags: self.flags,
            // on clone we have to reset the content lock
            content: OnceLock::new(),
        }
    }
}

impl FileItem {
    pub fn new_raw(
        filename_start: u16,
        size: u64,
        modified: u64,
        git_status: Option<git2::Status>,
        is_binary: bool,
    ) -> Self {
        let mut flags = 0u8;
        if is_binary {
            flags |= FileItemFlags::BINARY;
        }

        let mut path = crate::simd_path::ChunkedString::empty();
        path.filename_offset = filename_start;

        Self {
            path,
            parent_dir: u32::MAX,
            size,
            modified,
            access_frecency_score: 0,
            modification_frecency_score: 0,
            git_status,
            flags,
            content: OnceLock::new(),
        }
    }

    /// Returns an absolute path of the file
    pub fn absolute_path(&self, arena: impl FFFStringStorage, base_path: &Path) -> PathBuf {
        let mut buf = [0u8; PATH_BUF_SIZE];
        let rel = self.path.read_to_buf(arena.arena_for(self), &mut buf);
        base_path.join(rel)
    }

    pub(crate) fn set_path(&mut self, path: crate::simd_path::ChunkedString) {
        self.path = path;
    }

    pub(crate) fn parent_dir_index(&self) -> u32 {
        self.parent_dir
    }

    pub(crate) fn set_parent_dir(&mut self, idx: u32) {
        self.parent_dir = idx;
    }

    pub fn dir_str(&self, arena: impl FFFStringStorage) -> String {
        let mut s = String::with_capacity(64);
        self.path.write_dir_to(arena.arena_for(self), &mut s);
        s
    }

    pub(crate) fn write_dir_str(&self, arena: ArenaPtr, out: &mut String) {
        self.path.write_dir_to(arena, out);
    }

    pub fn file_name(&self, arena: impl FFFStringStorage) -> String {
        let mut s = String::with_capacity(32);
        self.path.write_filename_to(arena.arena_for(self), &mut s);
        s
    }

    pub(crate) fn write_file_name_from_arena(&self, arena: ArenaPtr, out: &mut String) {
        self.path.write_filename_to(arena, out);
    }

    pub fn relative_path(&self, arena: impl FFFStringStorage) -> String {
        let mut s = String::with_capacity(64);
        self.path.write_to_string(arena.arena_for(self), &mut s);
        s
    }

    pub(crate) fn write_relative_path_from_arena(&self, arena: ArenaPtr, out: &mut String) {
        self.path.write_to_string(arena, out);
    }

    pub fn relative_path_len(&self) -> usize {
        self.path.byte_len as usize
    }

    pub fn filename_offset_in_relative_path(&self) -> usize {
        self.path.filename_offset as usize
    }

    pub(crate) fn relative_path_eq(&self, arena: ArenaPtr, other: &str) -> bool {
        if other.len() != self.path.byte_len as usize {
            return false;
        }
        let mut buf = [0u8; 512];
        let mine = self.path.read_to_buf(arena, &mut buf);
        mine == other
    }

    pub(crate) fn relative_path_starts_with(&self, arena: ArenaPtr, prefix: &str) -> bool {
        let mut buf = [0u8; PATH_BUF_SIZE];
        let path = self.path.read_to_buf(arena, &mut buf);
        path.starts_with(prefix)
    }

    pub(crate) fn write_absolute_path<'a>(
        &self,
        arena: ArenaPtr,
        base_path: &Path,
        buf: &'a mut [u8; PATH_BUF_SIZE],
    ) -> &'a Path {
        let base = base_path.as_os_str().as_encoded_bytes();
        let base_len = base.len();
        buf[..base_len].copy_from_slice(base);
        // Add separator if base doesn't end with one
        let sep_len = if base_len > 0 && base[base_len - 1] != b'/' {
            buf[base_len] = b'/';
            1
        } else {
            0
        };
        let rel_start = base_len + sep_len;
        let mut rel_buf = [0u8; PATH_BUF_SIZE];
        let rel = self.path.read_to_buf(arena, &mut rel_buf);
        let rel_bytes = rel.as_bytes();
        buf[rel_start..rel_start + rel_bytes.len()].copy_from_slice(rel_bytes);
        let total = rel_start + rel_bytes.len();
        Path::new(unsafe { std::str::from_utf8_unchecked(&buf[..total]) })
    }

    #[inline]
    pub fn total_frecency_score(&self) -> i32 {
        self.access_frecency_score as i32 + self.modification_frecency_score as i32
    }

    #[inline]
    pub fn is_binary(&self) -> bool {
        self.flags & FileItemFlags::BINARY != 0
    }

    #[inline]
    pub fn set_binary(&mut self, val: bool) {
        if val {
            self.flags |= FileItemFlags::BINARY;
        } else {
            self.flags &= !FileItemFlags::BINARY;
        }
    }

    #[inline]
    pub fn is_deleted(&self) -> bool {
        self.flags & FileItemFlags::DELETED != 0
    }

    #[inline]
    pub fn set_deleted(&mut self, val: bool) {
        if val {
            self.flags |= FileItemFlags::DELETED;
        } else {
            self.flags &= !FileItemFlags::DELETED;
        }
    }

    #[inline]
    pub fn is_overflow(&self) -> bool {
        self.flags & FileItemFlags::OVERFLOW != 0
    }

    #[inline]
    pub fn set_overflow(&mut self, val: bool) {
        if val {
            self.flags |= FileItemFlags::OVERFLOW;
        } else {
            self.flags &= !FileItemFlags::OVERFLOW;
        }
    }
}

impl FileItem {
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
    pub(crate) fn get_content(
        &self,
        arena: ArenaPtr,
        base_path: &Path,
        budget: &ContentCacheBudget,
    ) -> Option<&[u8]> {
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

        let content = load_file_content(&self.absolute_path(arena, base_path), self.size)?;
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
    /// The caller provides a reusable `path_buf` (pre-filled with `base_path/`)
    /// and its `base_len` to avoid allocations when constructing the absolute path.
    #[inline]
    pub(crate) fn get_content_for_search<'a>(
        &'a self,
        buf: &'a mut Vec<u8>, // we allow it to grow
        arena: ArenaPtr,
        base_path: &Path,
        budget: &ContentCacheBudget,
    ) -> Option<&'a [u8]> {
        // Fast path: persistent cache hit (zero-copy).
        if let Some(cached) = self.get_content(arena, base_path, budget) {
            return Some(cached);
        }

        let max_file_size = budget.max_file_size;
        if self.is_binary() || self.size == 0 || self.size > max_file_size {
            return None;
        }

        // Slow path: read into the reusable buffer — open() + read_exact() + close().
        // No mmap()/munmap() syscalls, no page table setup/teardown.
        // We know the exact size so we use read_exact (1 read syscall) instead of
        // read_to_end (2 read syscalls — one for data, one for EOF confirmation).
        let abs = self.absolute_path(arena, base_path);
        let len = self.size as usize;
        buf.resize(len, 0);
        let mut file = std::fs::File::open(&abs).ok()?;
        file.read_exact(buf).ok()?;
        Some(buf.as_slice())
    }
}

/// Files smaller than one page waste the remainder when mmapped.
#[cfg(target_arch = "aarch64")]
const MMAP_THRESHOLD: u64 = 16 * 1024;
#[cfg(not(target_arch = "aarch64"))]
const MMAP_THRESHOLD: u64 = 4 * 1024;

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
    fn write_file_name(&self, arena: ArenaPtr, out: &mut String) {
        self.path.write_filename_to(arena, out);
    }

    #[inline]
    fn write_relative_path(&self, arena: ArenaPtr, out: &mut String) {
        self.path.write_to_string(arena, out);
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
    pub path_alignment_bonus: i32,
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

#[derive(Debug, Clone)]
pub struct ScoringContext<'a> {
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

/// Search result for directory-only fuzzy search.
#[derive(Debug, Clone, Default)]
pub struct DirSearchResult<'a> {
    pub items: Vec<&'a DirItem>,
    pub scores: Vec<Score>,
    pub total_matched: usize,
    pub total_dirs: usize,
}

/// A single item in a mixed (files + directories) search result.
#[derive(Debug, Clone)]
pub enum MixedItemRef<'a> {
    File(&'a FileItem),
    Dir(&'a DirItem),
}

/// Search result for mixed (files + directories) fuzzy search.
/// Items are interleaved by total score in descending order.
#[derive(Debug, Clone, Default)]
pub struct MixedSearchResult<'a> {
    pub items: Vec<MixedItemRef<'a>>,
    pub scores: Vec<Score>,
    pub total_matched: usize,
    pub total_files: usize,
    pub total_dirs: usize,
    pub location: Option<Location>,
}

impl Default for MixedItemRef<'_> {
    fn default() -> Self {
        // Should never be used, exists only for Default derive on MixedSearchResult
        unreachable!("MixedItemRef::default should not be called")
    }
}

const MAX_MMAP_FILE_SIZE: u64 = 10 * 1024 * 1024;

const MAX_CACHED_CONTENT_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug)]
pub struct ContentCacheBudget {
    pub max_files: usize,
    pub max_bytes: u64,
    pub max_file_size: u64,
    pub cached_count: AtomicUsize,
    pub cached_bytes: AtomicU64,
}

impl ContentCacheBudget {
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

#[cfg(test)]
impl FileItem {
    /// Leaks a single-file arena so the pointer stays valid forever.
    pub fn new_for_test(
        rel_path: &str,
        size: u64,
        modified: u64,
        git_status: Option<git2::Status>,
        is_binary: bool,
    ) -> Self {
        let (item, _arena) =
            Self::new_for_test_with_arena(rel_path, size, modified, git_status, is_binary);
        item
    }

    pub(crate) fn new_for_test_with_arena(
        rel_path: &str,
        size: u64,
        modified: u64,
        git_status: Option<git2::Status>,
        is_binary: bool,
    ) -> (Self, ArenaPtr) {
        let filename_start = rel_path.rfind('/').map(|i| i + 1).unwrap_or(0) as u16;
        let mut item = Self::new_raw(filename_start, size, modified, git_status, is_binary);
        let paths = [rel_path.to_string()];
        let (store, strings) = crate::simd_path::build_chunked_path_store_from_strings(
            &paths,
            std::slice::from_ref(&item),
        );
        let cs = strings.into_iter().next().unwrap();
        let arena = store.as_arena_ptr();
        item.set_path(cs);
        std::mem::forget(store);
        (item, arena)
    }
}
