/// Largest file whose full content fff will touch: the default grep read cap
/// (`GrepSearchOptions::max_file_size`) and the content-cache mmap cap
/// (`ContentCacheBudget::max_file_size`). Binary detection also streams up to
/// this far so nothing grep would read is left unclassified.
pub const MAX_FFFILE_SIZE: u64 = 10 * 1024 * 1024;

/// Upper bound on a file the bigram builder will build, if the file is very large there is a
/// big probability it will only bloat the available bigrams and will anyway pop ut from the prefilter
pub const MAX_INDEXABLE_FILE_SIZE: usize = 2 * 1024 * 1024;

/// Total bytes the persistent content mmap cache may hold for a small repo.
pub const MAX_CACHED_CONTENT_BYTES: u64 = 512 * 1024 * 1024;

/// Files below one page waste the remainder when mmapped, so the cache skips
/// them and falls back to chunked reads. Unused on Windows (no content cache).
#[cfg(all(not(target_os = "windows"), target_arch = "aarch64"))]
pub const MMAP_THRESHOLD: u64 = 16 * 1024;
#[cfg(all(not(target_os = "windows"), not(target_arch = "aarch64")))]
pub const MMAP_THRESHOLD: u64 = 4 * 1024;

/// Capacity reserved for files the watcher discovers after the initial scan;
/// exceeding it forces a full rescan.
pub const MAX_OVERFLOW_FILES: usize = 1024;

/// Fresh-mmap threshold: files at or above this size get mmapped directly on
/// cache miss instead of chunked reads into Vec. Empirically tuned per-platform.
/// Only referenced on Unix; Windows uses the `std::fs::read` fallback so this
/// constant is gated to non-Windows targets to keep `-D unused-imports` happy.
#[cfg(target_os = "macos")]
pub const FRESH_MMAP_THRESHOLD: u64 = 1024 * 1024;
#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
pub const FRESH_MMAP_THRESHOLD: u64 = 256 * 1024;

// we do not support 32kb path limit on windows
#[cfg(target_os = "windows")]
pub const PATH_BUF_SIZE: usize = 4096;

#[cfg(not(target_os = "windows"))]
pub const PATH_BUF_SIZE: usize = libc::PATH_MAX as usize;
