use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::constraints::Constrainable;
use crate::query_tracker::QueryMatchEntry;
use fff_query_parser::{FFFQuery, FuzzyQuery, Location};

/// Cached file contents — mmap on Unix, heap buffer on Windows.
///
/// On Windows, memory-mapped files hold the file handle open and prevent
/// editors from saving (writing/replacing) those files. Reading into a
/// `Vec<u8>` releases the handle immediately after the read completes.
#[derive(Debug)]
#[allow(dead_code)] // variants are conditionally used per platform
enum FileContent {
    #[cfg(not(target_os = "windows"))]
    Mmap(memmap2::Mmap),
    #[cfg(target_os = "windows")]
    Buffer(Vec<u8>),
}

impl std::ops::Deref for FileContent {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            #[cfg(not(target_os = "windows"))]
            FileContent::Mmap(m) => m,
            #[cfg(target_os = "windows")]
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
    pub relative_path_lower: String,
    pub file_name: String,
    pub file_name_lower: String,
    pub size: u64,
    pub modified: u64,
    pub access_frecency_score: i64,
    pub modification_frecency_score: i64,
    pub total_frecency_score: i64,
    pub git_status: Option<git2::Status>,
    pub is_binary: bool,
    /// Lazily-initialized file contents for grep.
    /// Initialized on first grep access via `OnceLock`; lock-free on subsequent reads.
    content: OnceLock<FileContent>,
}

impl Clone for FileItem {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            relative_path: self.relative_path.clone(),
            relative_path_lower: self.relative_path_lower.clone(),
            file_name: self.file_name.clone(),
            file_name_lower: self.file_name_lower.clone(),
            size: self.size,
            modified: self.modified,
            access_frecency_score: self.access_frecency_score,
            modification_frecency_score: self.modification_frecency_score,
            total_frecency_score: self.total_frecency_score,
            git_status: self.git_status,
            is_binary: self.is_binary,
            // Don't clone the content — the clone lazily re-creates it on demand
            content: OnceLock::new(),
        }
    }
}

const MAX_MMAP_FILE_SIZE: u64 = 10 * 1024 * 1024;

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
            relative_path_lower: relative_path.to_lowercase(),
            file_name_lower: file_name.to_lowercase(),
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
            content: OnceLock::new(),
        }
    }

    /// Invalidate the cached content so the next `get_content()` call creates a fresh one.
    ///
    /// Call this when the background watcher detects that the file has been modified.
    /// On Unix, a file that is truncated while mapped can cause SIGBUS. On Windows,
    /// the stale buffer simply won't reflect the new contents. In both cases,
    /// invalidating ensures a fresh read on the next access.
    pub fn invalidate_mmap(&mut self) {
        self.content = OnceLock::new();
    }

    /// Get the cached file contents or lazily load them. Returns `None` if the
    /// file is too large, empty, or can't be opened.
    ///
    /// After the first call, this is lock-free (just an atomic load + pointer deref).
    /// On Unix, uses mmap backed by the kernel page cache. On Windows, reads into
    /// a heap buffer so the file handle is released immediately.
    #[inline]
    pub fn get_content(&self) -> Option<&[u8]> {
        if let Some(content) = self.content.get() {
            return Some(content);
        }

        if self.size == 0 || self.size > MAX_MMAP_FILE_SIZE {
            return None;
        }

        let content = load_file_content(&self.path)?;

        // If another thread raced us, OnceLock discards ours and returns theirs.
        Some(self.content.get_or_init(|| content))
    }

    /// Backward-compatible alias for `get_content`.
    #[inline]
    pub fn get_mmap(&self) -> Option<&[u8]> {
        self.get_content()
    }
}

/// Load file contents: mmap on Unix, heap buffer on Windows.
fn load_file_content(path: &Path) -> Option<FileContent> {
    #[cfg(not(target_os = "windows"))]
    {
        let file = std::fs::File::open(path).ok()?;
        // SAFETY: The mmap is backed by the kernel page cache and automatically
        // reflects file modifications. The only risk is SIGBUS if the file is
        // truncated while mapped.
        let mmap = unsafe { memmap2::Mmap::map(&file) }.ok()?;
        Some(FileContent::Mmap(mmap))
    }

    #[cfg(target_os = "windows")]
    {
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
    fn relative_path_lower(&self) -> &str {
        &self.relative_path_lower
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

#[derive(Debug, Clone)]
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

/// Context for scoring files during search.
///
/// The `parsed_query` field contains the pre-parsed query with constraints,
/// fuzzy parts, and location information. Parsing is done once at the API
/// boundary and passed through.
#[derive(Debug, Clone)]
pub struct ScoringContext<'a> {
    /// The original raw query string (for compatibility and debugging)
    pub raw_query: &'a str,
    /// Pre-parsed query containing constraints, fuzzy parts, and location
    pub parsed_query: Option<FFFQuery<'a>>,
    pub project_path: Option<&'a Path>,
    pub current_file: Option<&'a str>,
    pub max_typos: u16,
    pub max_threads: usize,
    pub last_same_query_match: Option<&'a QueryMatchEntry>,
    pub combo_boost_score_multiplier: i32,
    pub min_combo_count: u32,
    pub pagination: PaginationArgs,
}

impl<'a> ScoringContext<'a> {
    /// Get the effective fuzzy query string for matching.
    /// Returns the first fuzzy part, or the raw query if no parsing was done.
    pub fn effective_query(&self) -> &'a str {
        match &self.parsed_query {
            Some(p) => match &p.fuzzy_query {
                FuzzyQuery::Text(t) => t,
                FuzzyQuery::Parts(parts) if !parts.is_empty() => parts[0],
                _ => self.raw_query.trim(),
            },
            None => self.raw_query.trim(),
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
