//! Python bindings for FFF.

#![allow(unsafe_op_in_unsafe_fn)]

use std::path::PathBuf;
use std::time::Duration;

use pyo3::create_exception;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use fff::file_picker::FilePicker;
use fff::frecency::FrecencyTracker;
use fff::git::format_git_status;
use fff::grep::{GrepMatch as CoreGrepMatch, GrepResult as CoreGrepResult, GrepSearchOptions};
use fff::query_tracker::QueryTracker;
use fff::shared::{SharedFilePicker, SharedFrecency, SharedQueryTracker};
use fff::types::ContentCacheBudget;
use fff::{
    DirItem as CoreDirItem, DirSearchResult as CoreDirSearchResult, FFFMode, FilePickerOptions,
    FileItem as CoreFileItem, FuzzySearchOptions, Location as CoreLocation,
    MixedItemRef as CoreMixedItemRef, MixedSearchResult as CoreMixedSearchResult,
    PaginationArgs, QueryParser, Score as CoreScore, SearchResult as CoreSearchResult,
};

create_exception!(fff_search, FffError, PyRuntimeError);

fn err(msg: impl Into<String>) -> PyErr {
    PyErr::new::<FffError, _>(msg.into())
}

fn default_or<T: Default + PartialEq + Copy>(val: T, default: T) -> T {
    if val == T::default() { default } else { val }
}

fn grep_mode_from_str(s: &str) -> PyResult<fff::GrepMode> {
    match s {
        "plain" | "" => Ok(fff::GrepMode::PlainText),
        "regex" => Ok(fff::GrepMode::Regex),
        "fuzzy" => Ok(fff::GrepMode::Fuzzy),
        other => Err(err(format!(
            "invalid grep mode: {other:?} (expected 'plain', 'regex', or 'fuzzy')"
        ))),
    }
}

#[pyclass(module = "fff_search._native", frozen, get_all)]
#[derive(Clone, Debug)]
pub struct FileItem {
    pub relative_path: String,
    pub file_name: String,
    pub git_status: String,
    pub size: u64,
    pub modified: u64,
    pub access_frecency_score: i64,
    pub modification_frecency_score: i64,
    pub total_frecency_score: i64,
    pub is_binary: bool,
}

#[pymethods]
impl FileItem {
    fn __repr__(&self) -> String {
        format!(
            "FileItem(relative_path={:?}, size={}, total_frecency_score={}, is_binary={})",
            self.relative_path, self.size, self.total_frecency_score, self.is_binary
        )
    }
}

impl FileItem {
    fn from_core(item: &CoreFileItem, picker: &FilePicker) -> Self {
        FileItem {
            relative_path: item.relative_path(picker).to_string(),
            file_name: item.file_name(picker).to_string(),
            git_status: format_git_status(item.git_status).to_string(),
            size: item.size,
            modified: item.modified,
            access_frecency_score: item.access_frecency_score as i64,
            modification_frecency_score: item.modification_frecency_score as i64,
            total_frecency_score: item.total_frecency_score() as i64,
            is_binary: item.is_binary(),
        }
    }
}

#[pyclass(module = "fff_search._native", frozen, get_all)]
#[derive(Clone, Debug)]
pub struct Score {
    pub total: i32,
    pub base_score: i32,
    pub filename_bonus: i32,
    pub special_filename_bonus: i32,
    pub frecency_boost: i32,
    pub distance_penalty: i32,
    pub current_file_penalty: i32,
    pub combo_match_boost: i32,
    pub path_alignment_bonus: i32,
    pub exact_match: bool,
    pub match_type: String,
}

#[pymethods]
impl Score {
    fn __repr__(&self) -> String {
        format!(
            "Score(total={}, match_type={:?}, exact_match={})",
            self.total, self.match_type, self.exact_match
        )
    }
}

impl From<&CoreScore> for Score {
    fn from(s: &CoreScore) -> Self {
        Score {
            total: s.total,
            base_score: s.base_score,
            filename_bonus: s.filename_bonus,
            special_filename_bonus: s.special_filename_bonus,
            frecency_boost: s.frecency_boost,
            distance_penalty: s.distance_penalty,
            current_file_penalty: s.current_file_penalty,
            combo_match_boost: s.combo_match_boost,
            path_alignment_bonus: s.path_alignment_bonus,
            exact_match: s.exact_match,
            match_type: s.match_type.to_string(),
        }
    }
}

#[pyclass(module = "fff_search._native", frozen, get_all)]
#[derive(Clone, Debug)]
pub struct Location {
    /// "line" | "position" | "range"
    pub kind: String,
    pub line: i32,
    pub col: i32,
    pub end_line: i32,
    pub end_col: i32,
}

#[pymethods]
impl Location {
    fn __repr__(&self) -> String {
        match self.kind.as_str() {
            "line" => format!("Location(line={})", self.line),
            "position" => format!("Location(line={}, col={})", self.line, self.col),
            "range" => format!(
                "Location({}:{}..{}:{})",
                self.line, self.col, self.end_line, self.end_col
            ),
            _ => "Location(none)".to_string(),
        }
    }
}

impl Location {
    fn from_core(loc: Option<&CoreLocation>) -> Option<Self> {
        match loc? {
            CoreLocation::Line(line) => Some(Location {
                kind: "line".to_string(),
                line: *line,
                col: 0,
                end_line: 0,
                end_col: 0,
            }),
            CoreLocation::Position { line, col } => Some(Location {
                kind: "position".to_string(),
                line: *line,
                col: *col,
                end_line: 0,
                end_col: 0,
            }),
            CoreLocation::Range { start, end } => Some(Location {
                kind: "range".to_string(),
                line: start.0,
                col: start.1,
                end_line: end.0,
                end_col: end.1,
            }),
        }
    }
}

#[pyclass(module = "fff_search._native", frozen, get_all)]
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub items: Vec<FileItem>,
    pub scores: Vec<Score>,
    pub total_matched: u32,
    pub total_files: u32,
    pub location: Option<Location>,
}

#[pymethods]
impl SearchResult {
    fn __repr__(&self) -> String {
        format!(
            "SearchResult(items={}, total_matched={}, total_files={})",
            self.items.len(),
            self.total_matched,
            self.total_files
        )
    }

    fn __len__(&self) -> usize {
        self.items.len()
    }
}

impl SearchResult {
    fn from_core(r: &CoreSearchResult, picker: &FilePicker) -> Self {
        SearchResult {
            items: r
                .items
                .iter()
                .map(|i| FileItem::from_core(i, picker))
                .collect(),
            scores: r.scores.iter().map(Score::from).collect(),
            total_matched: r.total_matched as u32,
            total_files: r.total_files as u32,
            location: Location::from_core(r.location.as_ref()),
        }
    }
}

#[pyclass(module = "fff_search._native", frozen, get_all)]
#[derive(Clone, Debug)]
pub struct DirItem {
    pub relative_path: String,
    pub dir_name: String,
    pub max_access_frecency: i32,
}

#[pymethods]
impl DirItem {
    fn __repr__(&self) -> String {
        format!("DirItem(relative_path={:?})", self.relative_path)
    }
}

impl DirItem {
    fn from_core(d: &CoreDirItem, picker: &FilePicker) -> Self {
        DirItem {
            relative_path: d.relative_path(picker).to_string(),
            dir_name: d.dir_name(picker).to_string(),
            max_access_frecency: d.max_access_frecency(),
        }
    }
}

#[pyclass(module = "fff_search._native", frozen, get_all)]
#[derive(Clone, Debug)]
pub struct DirSearchResult {
    pub items: Vec<DirItem>,
    pub scores: Vec<Score>,
    pub total_matched: u32,
    pub total_dirs: u32,
}

#[pymethods]
impl DirSearchResult {
    fn __repr__(&self) -> String {
        format!(
            "DirSearchResult(items={}, total_matched={}, total_dirs={})",
            self.items.len(),
            self.total_matched,
            self.total_dirs
        )
    }

    fn __len__(&self) -> usize {
        self.items.len()
    }
}

impl DirSearchResult {
    fn from_core(r: &CoreDirSearchResult, picker: &FilePicker) -> Self {
        DirSearchResult {
            items: r
                .items
                .iter()
                .map(|d| DirItem::from_core(d, picker))
                .collect(),
            scores: r.scores.iter().map(Score::from).collect(),
            total_matched: r.total_matched as u32,
            total_dirs: r.total_dirs as u32,
        }
    }
}

#[pyclass(module = "fff_search._native", frozen, get_all)]
#[derive(Clone, Debug)]
pub struct MixedItem {
    /// "file" or "directory"
    pub kind: String,
    pub file: Option<FileItem>,
    pub directory: Option<DirItem>,
}

#[pymethods]
impl MixedItem {
    fn __repr__(&self) -> String {
        match self.kind.as_str() {
            "file" => format!("MixedItem(file={:?})", self.file),
            "directory" => format!("MixedItem(directory={:?})", self.directory),
            _ => "MixedItem(unknown)".to_string(),
        }
    }
}

#[pyclass(module = "fff_search._native", frozen, get_all)]
#[derive(Clone, Debug)]
pub struct MixedSearchResult {
    pub items: Vec<MixedItem>,
    pub scores: Vec<Score>,
    pub total_matched: u32,
    pub total_files: u32,
    pub total_dirs: u32,
    pub location: Option<Location>,
}

#[pymethods]
impl MixedSearchResult {
    fn __repr__(&self) -> String {
        format!(
            "MixedSearchResult(items={}, total_matched={}, total_files={}, total_dirs={})",
            self.items.len(),
            self.total_matched,
            self.total_files,
            self.total_dirs
        )
    }

    fn __len__(&self) -> usize {
        self.items.len()
    }
}

impl MixedSearchResult {
    fn from_core(r: &CoreMixedSearchResult, picker: &FilePicker) -> Self {
        let items = r
            .items
            .iter()
            .map(|item| match item {
                CoreMixedItemRef::File(f) => MixedItem {
                    kind: "file".to_string(),
                    file: Some(FileItem::from_core(f, picker)),
                    directory: None,
                },
                CoreMixedItemRef::Dir(d) => MixedItem {
                    kind: "directory".to_string(),
                    file: None,
                    directory: Some(DirItem::from_core(d, picker)),
                },
            })
            .collect();
        MixedSearchResult {
            items,
            scores: r.scores.iter().map(Score::from).collect(),
            total_matched: r.total_matched as u32,
            total_files: r.total_files as u32,
            total_dirs: r.total_dirs as u32,
            location: Location::from_core(r.location.as_ref()),
        }
    }
}

#[pyclass(module = "fff_search._native", frozen, get_all)]
#[derive(Clone, Debug)]
pub struct GrepMatch {
    pub relative_path: String,
    pub file_name: String,
    pub git_status: String,
    pub line_content: String,
    pub match_ranges: Vec<(u32, u32)>,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
    pub size: u64,
    pub modified: u64,
    pub total_frecency_score: i64,
    pub access_frecency_score: i64,
    pub modification_frecency_score: i64,
    pub line_number: u64,
    pub byte_offset: u64,
    pub col: u32,
    pub fuzzy_score: Option<u16>,
    pub is_binary: bool,
    pub is_definition: bool,
}

#[pymethods]
impl GrepMatch {
    fn __repr__(&self) -> String {
        format!(
            "GrepMatch({}:{}: {:?})",
            self.relative_path,
            self.line_number,
            if self.line_content.len() > 60 {
                format!("{}…", &self.line_content[..60])
            } else {
                self.line_content.clone()
            }
        )
    }
}

impl GrepMatch {
    fn from_core(m: &CoreGrepMatch, file: &CoreFileItem, picker: &FilePicker) -> Self {
        GrepMatch {
            relative_path: file.relative_path(picker).to_string(),
            file_name: file.file_name(picker).to_string(),
            git_status: format_git_status(file.git_status).to_string(),
            line_content: m.line_content.clone(),
            match_ranges: m
                .match_byte_offsets
                .iter()
                .map(|r| (r.0, r.1))
                .collect(),
            context_before: m.context_before.clone(),
            context_after: m.context_after.clone(),
            size: file.size,
            modified: file.modified,
            total_frecency_score: file.total_frecency_score() as i64,
            access_frecency_score: file.access_frecency_score as i64,
            modification_frecency_score: file.modification_frecency_score as i64,
            line_number: m.line_number,
            byte_offset: m.byte_offset,
            col: m.col as u32,
            fuzzy_score: m.fuzzy_score,
            is_binary: file.is_binary(),
            is_definition: m.is_definition,
        }
    }
}

#[pyclass(module = "fff_search._native", frozen, get_all)]
#[derive(Clone, Debug)]
pub struct GrepResult {
    pub items: Vec<GrepMatch>,
    pub total_matched: u32,
    pub total_files_searched: u32,
    pub total_files: u32,
    pub filtered_file_count: u32,
    /// Pass back as `cursor=` to fetch the next page; `None` when done.
    pub next_cursor: Option<u32>,
    pub regex_fallback_error: Option<String>,
}

#[pymethods]
impl GrepResult {
    fn __repr__(&self) -> String {
        format!(
            "GrepResult(items={}, total_files_searched={}, next_cursor={:?})",
            self.items.len(),
            self.total_files_searched,
            self.next_cursor
        )
    }

    fn __len__(&self) -> usize {
        self.items.len()
    }
}

impl GrepResult {
    fn from_core(r: &CoreGrepResult<'_>, picker: &FilePicker) -> Self {
        let items: Vec<GrepMatch> = r
            .matches
            .iter()
            .filter_map(|m| {
                let file: &CoreFileItem = *r.files.get(m.file_index)?;
                Some(GrepMatch::from_core(m, file, picker))
            })
            .collect();
        let total_matched = items.len() as u32;
        GrepResult {
            items,
            total_matched,
            total_files_searched: r.total_files_searched as u32,
            total_files: r.total_files as u32,
            filtered_file_count: r.filtered_file_count as u32,
            next_cursor: if r.next_file_offset == 0 {
                None
            } else {
                Some(r.next_file_offset as u32)
            },
            regex_fallback_error: r.regex_fallback_error.clone(),
        }
    }
}

#[pyclass(module = "fff_search._native", frozen, get_all)]
#[derive(Clone, Debug)]
pub struct ScanProgress {
    pub scanned_files_count: u64,
    pub is_scanning: bool,
    pub is_watcher_ready: bool,
    pub is_warmup_complete: bool,
}

#[pymethods]
impl ScanProgress {
    fn __repr__(&self) -> String {
        format!(
            "ScanProgress(scanned_files_count={}, is_scanning={}, is_watcher_ready={}, is_warmup_complete={})",
            self.scanned_files_count,
            self.is_scanning,
            self.is_watcher_ready,
            self.is_warmup_complete
        )
    }
}

#[pyclass(module = "fff_search._native")]
pub struct FileFinder {
    picker: SharedFilePicker,
    frecency: SharedFrecency,
    query_tracker: SharedQueryTracker,
    destroyed: bool,
}

#[pymethods]
impl FileFinder {
    /// Create a new file finder rooted at `base_path`.
    #[staticmethod]
    #[pyo3(signature = (
        base_path,
        *,
        frecency_db_path = None,
        history_db_path = None,
        disable_mmap_cache = false,
        disable_content_indexing = None,
        disable_watch = false,
        ai_mode = false,
        log_file_path = None,
        log_level = None,
        cache_budget_max_files = 0,
        cache_budget_max_bytes = 0,
        cache_budget_max_file_size = 0,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn create(
        base_path: String,
        frecency_db_path: Option<String>,
        history_db_path: Option<String>,
        disable_mmap_cache: bool,
        disable_content_indexing: Option<bool>,
        disable_watch: bool,
        ai_mode: bool,
        log_file_path: Option<String>,
        log_level: Option<String>,
        cache_budget_max_files: u64,
        cache_budget_max_bytes: u64,
        cache_budget_max_file_size: u64,
    ) -> PyResult<Self> {
        if base_path.is_empty() {
            return Err(err("base_path is required and cannot be empty"));
        }

        if let Some(log_path) = log_file_path.as_deref() {
            let level = log_level.as_deref();
            fff::log::init_tracing(log_path, level)
                .map_err(|e| err(format!("Failed to init tracing: {e}")))?;
        }

        let shared_picker = SharedFilePicker::default();
        let shared_frecency = SharedFrecency::default();
        let shared_query_tracker = SharedQueryTracker::default();

        if let Some(p) = frecency_db_path.as_deref() {
            if !p.is_empty() {
                if let Some(parent) = PathBuf::from(p).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let tracker = FrecencyTracker::open(p)
                    .map_err(|e| err(format!("Failed to init frecency db: {e}")))?;
                shared_frecency
                    .init(tracker)
                    .map_err(|e| err(format!("Failed to acquire frecency lock: {e}")))?;
                let _ = shared_frecency.spawn_gc(p.to_string());
            }
        }

        if let Some(p) = history_db_path.as_deref() {
            if !p.is_empty() {
                if let Some(parent) = PathBuf::from(p).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let tracker = QueryTracker::open(p)
                    .map_err(|e| err(format!("Failed to init query tracker db: {e}")))?;
                shared_query_tracker
                    .init(tracker)
                    .map_err(|e| err(format!("Failed to acquire query tracker lock: {e}")))?;
            }
        }

        let mode = if ai_mode { FFFMode::Ai } else { FFFMode::Neovim };

        let cache_budget = ContentCacheBudget::from_overrides(
            cache_budget_max_files as usize,
            cache_budget_max_bytes,
            cache_budget_max_file_size,
        );

        let enable_mmap_cache = !disable_mmap_cache;
        let enable_content_indexing = !disable_content_indexing.unwrap_or(disable_mmap_cache);
        let watch = !disable_watch;

        FilePicker::new_with_shared_state(
            shared_picker.clone(),
            shared_frecency.clone(),
            FilePickerOptions {
                base_path,
                enable_mmap_cache,
                enable_content_indexing,
                watch,
                mode,
                cache_budget,
            },
        )
        .map_err(|e| err(format!("Failed to init file picker: {e}")))?;

        Ok(FileFinder {
            picker: shared_picker,
            frecency: shared_frecency,
            query_tracker: shared_query_tracker,
            destroyed: false,
        })
    }

    /// Tear down the picker, watcher, and databases. Idempotent.
    fn destroy(&mut self) {
        if self.destroyed {
            return;
        }
        if let Ok(mut guard) = self.picker.write() {
            if let Some(mut p) = guard.take() {
                p.stop_background_monitor();
            }
        }
        if let Ok(mut g) = self.frecency.write() {
            *g = None;
        }
        if let Ok(mut g) = self.query_tracker.write() {
            *g = None;
        }
        self.destroyed = true;
    }

    #[getter]
    fn is_destroyed(&self) -> bool {
        self.destroyed
    }

    fn __enter__<'py>(slf: Bound<'py, Self>) -> Bound<'py, Self> {
        slf
    }

    fn __exit__(
        &mut self,
        _exc_type: Option<PyObject>,
        _exc_val: Option<PyObject>,
        _exc_tb: Option<PyObject>,
    ) -> bool {
        self.destroy();
        false
    }

    /// Block until the initial scan finishes or `timeout_ms` elapses.
    fn wait_for_scan(&self, py: Python<'_>, timeout_ms: u64) -> PyResult<bool> {
        self.ensure_alive()?;
        let dur = Duration::from_millis(timeout_ms);
        Ok(py.allow_threads(|| self.picker.wait_for_scan(dur)))
    }

    /// Block until the background watcher is ready or `timeout_ms` elapses.
    fn wait_for_watcher(&self, py: Python<'_>, timeout_ms: u64) -> PyResult<bool> {
        self.ensure_alive()?;
        let dur = Duration::from_millis(timeout_ms);
        Ok(py.allow_threads(|| self.picker.wait_for_watcher(dur)))
    }

    fn is_scanning(&self) -> PyResult<bool> {
        self.ensure_alive()?;
        let guard = self
            .picker
            .read()
            .map_err(|e| err(format!("picker lock: {e}")))?;
        Ok(guard.as_ref().map(|p| p.is_scan_active()).unwrap_or(false))
    }

    fn get_scan_progress(&self) -> PyResult<ScanProgress> {
        self.ensure_alive()?;
        let guard = self
            .picker
            .read()
            .map_err(|e| err(format!("picker lock: {e}")))?;
        let p = guard
            .as_ref()
            .ok_or_else(|| err("picker not initialized"))?;
        let prog = p.get_scan_progress();
        Ok(ScanProgress {
            scanned_files_count: prog.scanned_files_count as u64,
            is_scanning: prog.is_scanning,
            is_watcher_ready: prog.is_watcher_ready,
            is_warmup_complete: prog.is_warmup_complete,
        })
    }

    /// Trigger an async full rescan.
    fn scan_files(&self) -> PyResult<()> {
        self.ensure_alive()?;
        self.picker
            .trigger_full_rescan_async(&self.frecency)
            .map_err(|e| err(format!("scan_files failed: {e}")))?;
        Ok(())
    }

    /// Refresh git status. Returns the number of files updated.
    fn refresh_git_status(&self) -> PyResult<usize> {
        self.ensure_alive()?;
        self.picker
            .refresh_git_status(&self.frecency)
            .map_err(|e| err(format!("refresh_git_status failed: {e}")))
    }

    fn get_base_path(&self) -> PyResult<Option<String>> {
        self.ensure_alive()?;
        let guard = self
            .picker
            .read()
            .map_err(|e| err(format!("picker lock: {e}")))?;
        Ok(guard.as_ref().map(|p| p.base_path().display().to_string()))
    }

    /// Fuzzy-search indexed files.
    #[pyo3(signature = (
        query,
        *,
        current_file = None,
        max_threads = 0,
        page_index = 0,
        page_size = 0,
        combo_boost_multiplier = 0,
        min_combo_count = 0,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn file_search(
        &self,
        py: Python<'_>,
        query: &str,
        current_file: Option<&str>,
        max_threads: u32,
        page_index: u32,
        page_size: u32,
        combo_boost_multiplier: i32,
        min_combo_count: u32,
    ) -> PyResult<SearchResult> {
        self.ensure_alive()?;
        let page_size = default_or(page_size, 100) as usize;
        let min_combo_count = default_or(min_combo_count, 3);
        let combo_boost_multiplier = default_or(combo_boost_multiplier, 100);

        py.allow_threads(|| -> PyResult<SearchResult> {
            let guard = self
                .picker
                .read()
                .map_err(|e| err(format!("picker lock: {e}")))?;
            let picker = guard.as_ref().ok_or_else(|| err("picker not initialized"))?;
            let qt_guard = self
                .query_tracker
                .read()
                .map_err(|_| err("query tracker lock"))?;

            let parser = QueryParser::default();
            let parsed = parser.parse(query);

            let results = picker.fuzzy_search(
                &parsed,
                qt_guard.as_ref(),
                FuzzySearchOptions {
                    max_threads: max_threads as usize,
                    current_file,
                    project_path: Some(picker.base_path()),
                    combo_boost_score_multiplier: combo_boost_multiplier,
                    min_combo_count,
                    pagination: PaginationArgs {
                        offset: page_index as usize,
                        limit: page_size,
                    },
                },
            );
            Ok(SearchResult::from_core(&results, picker))
        })
    }

    /// Fuzzy-search indexed directories.
    #[pyo3(signature = (
        query,
        *,
        current_file = None,
        max_threads = 0,
        page_index = 0,
        page_size = 0,
    ))]
    fn directory_search(
        &self,
        py: Python<'_>,
        query: &str,
        current_file: Option<&str>,
        max_threads: u32,
        page_index: u32,
        page_size: u32,
    ) -> PyResult<DirSearchResult> {
        self.ensure_alive()?;
        let page_size = default_or(page_size, 100) as usize;

        py.allow_threads(|| -> PyResult<DirSearchResult> {
            let guard = self
                .picker
                .read()
                .map_err(|e| err(format!("picker lock: {e}")))?;
            let picker = guard.as_ref().ok_or_else(|| err("picker not initialized"))?;

            let parser = QueryParser::new(fff_query_parser::DirSearchConfig);
            let parsed = parser.parse(query);

            let results = picker.fuzzy_search_directories(
                &parsed,
                FuzzySearchOptions {
                    max_threads: max_threads as usize,
                    current_file,
                    project_path: Some(picker.base_path()),
                    combo_boost_score_multiplier: 0,
                    min_combo_count: 0,
                    pagination: PaginationArgs {
                        offset: page_index as usize,
                        limit: page_size,
                    },
                },
            );
            Ok(DirSearchResult::from_core(&results, picker))
        })
    }

    /// Files + directories interleaved by score.
    #[pyo3(signature = (
        query,
        *,
        current_file = None,
        max_threads = 0,
        page_index = 0,
        page_size = 0,
        combo_boost_multiplier = 0,
        min_combo_count = 0,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn mixed_search(
        &self,
        py: Python<'_>,
        query: &str,
        current_file: Option<&str>,
        max_threads: u32,
        page_index: u32,
        page_size: u32,
        combo_boost_multiplier: i32,
        min_combo_count: u32,
    ) -> PyResult<MixedSearchResult> {
        self.ensure_alive()?;
        let page_size = default_or(page_size, 100) as usize;
        let min_combo_count = default_or(min_combo_count, 3);
        let combo_boost_multiplier = default_or(combo_boost_multiplier, 100);

        py.allow_threads(|| -> PyResult<MixedSearchResult> {
            let guard = self
                .picker
                .read()
                .map_err(|e| err(format!("picker lock: {e}")))?;
            let picker = guard.as_ref().ok_or_else(|| err("picker not initialized"))?;
            let qt_guard = self
                .query_tracker
                .read()
                .map_err(|_| err("query tracker lock"))?;

            let parser = QueryParser::new(fff_query_parser::MixedSearchConfig);
            let parsed = parser.parse(query);

            let results = picker.fuzzy_search_mixed(
                &parsed,
                qt_guard.as_ref(),
                FuzzySearchOptions {
                    max_threads: max_threads as usize,
                    current_file,
                    project_path: Some(picker.base_path()),
                    combo_boost_score_multiplier: combo_boost_multiplier,
                    min_combo_count,
                    pagination: PaginationArgs {
                        offset: page_index as usize,
                        limit: page_size,
                    },
                },
            );
            Ok(MixedSearchResult::from_core(&results, picker))
        })
    }

    /// Content grep. `mode` is `'plain' | 'regex' | 'fuzzy'`.
    #[pyo3(signature = (
        query,
        *,
        mode = "plain",
        max_file_size = 0,
        max_matches_per_file = 0,
        smart_case = true,
        cursor = None,
        page_limit = 0,
        time_budget_ms = 0,
        before_context = 0,
        after_context = 0,
        classify_definitions = false,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn grep(
        &self,
        py: Python<'_>,
        query: &str,
        mode: &str,
        max_file_size: u64,
        max_matches_per_file: u32,
        smart_case: bool,
        cursor: Option<u32>,
        page_limit: u32,
        time_budget_ms: u64,
        before_context: u32,
        after_context: u32,
        classify_definitions: bool,
    ) -> PyResult<GrepResult> {
        self.ensure_alive()?;
        let mode = grep_mode_from_str(mode)?;

        py.allow_threads(|| -> PyResult<GrepResult> {
            let guard = self
                .picker
                .read()
                .map_err(|e| err(format!("picker lock: {e}")))?;
            let picker = guard.as_ref().ok_or_else(|| err("picker not initialized"))?;

            let is_ai = picker.mode().is_ai();
            let parsed = if is_ai {
                QueryParser::new(fff_query_parser::AiGrepConfig).parse(query)
            } else {
                fff::grep::parse_grep_query(query)
            };

            let options = GrepSearchOptions {
                max_file_size: default_or(max_file_size, 10 * 1024 * 1024),
                max_matches_per_file: max_matches_per_file as usize,
                smart_case,
                file_offset: cursor.unwrap_or(0) as usize,
                page_limit: default_or(page_limit, 50) as usize,
                mode,
                time_budget_ms,
                before_context: before_context as usize,
                after_context: after_context as usize,
                classify_definitions,
                trim_whitespace: false,
                abort_signal: None,
            };

            let result = picker.grep(&parsed, &options);
            Ok(GrepResult::from_core(&result, picker))
        })
    }

    /// Multi-pattern OR grep (Aho-Corasick). `patterns` must be non-empty.
    #[pyo3(signature = (
        patterns,
        *,
        constraints = None,
        max_file_size = 0,
        max_matches_per_file = 0,
        smart_case = true,
        cursor = None,
        page_limit = 0,
        time_budget_ms = 0,
        before_context = 0,
        after_context = 0,
        classify_definitions = false,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn multi_grep(
        &self,
        py: Python<'_>,
        patterns: Vec<String>,
        constraints: Option<&str>,
        max_file_size: u64,
        max_matches_per_file: u32,
        smart_case: bool,
        cursor: Option<u32>,
        page_limit: u32,
        time_budget_ms: u64,
        before_context: u32,
        after_context: u32,
        classify_definitions: bool,
    ) -> PyResult<GrepResult> {
        self.ensure_alive()?;
        if patterns.is_empty() || patterns.iter().all(|p| p.is_empty()) {
            return Err(err("patterns must contain at least one non-empty string"));
        }

        py.allow_threads(|| -> PyResult<GrepResult> {
            let guard = self
                .picker
                .read()
                .map_err(|e| err(format!("picker lock: {e}")))?;
            let picker = guard.as_ref().ok_or_else(|| err("picker not initialized"))?;

            let is_ai = picker.mode().is_ai();

            let parsed_constraints = constraints.map(|c| {
                if is_ai {
                    QueryParser::new(fff_query_parser::AiGrepConfig).parse(c)
                } else {
                    fff::grep::parse_grep_query(c)
                }
            });
            let constraint_refs: &[fff::Constraint<'_>] = match &parsed_constraints {
                Some(q) => &q.constraints,
                None => &[],
            };

            let pattern_refs: Vec<&str> = patterns.iter().map(|s| s.as_str()).collect();

            let options = GrepSearchOptions {
                max_file_size: default_or(max_file_size, 10 * 1024 * 1024),
                max_matches_per_file: max_matches_per_file as usize,
                smart_case,
                file_offset: cursor.unwrap_or(0) as usize,
                page_limit: default_or(page_limit, 50) as usize,
                mode: fff::GrepMode::PlainText,
                time_budget_ms,
                before_context: before_context as usize,
                after_context: after_context as usize,
                classify_definitions,
                trim_whitespace: false,
                abort_signal: None,
            };

            let result = picker.multi_grep(&pattern_refs, constraint_refs, &options);
            Ok(GrepResult::from_core(&result, picker))
        })
    }

    /// Record a query→file selection. Requires the query tracker DB.
    fn track_query(&self, query: &str, selected_file_path: &str) -> PyResult<bool> {
        self.ensure_alive()?;
        let project_path = {
            let pg = self
                .picker
                .read()
                .map_err(|e| err(format!("picker lock: {e}")))?;
            pg.as_ref().map(|p| p.base_path().to_path_buf())
        };
        let project_path = match project_path {
            Some(p) => p,
            None => return Ok(false),
        };
        let mut qt_guard = self
            .query_tracker
            .write()
            .map_err(|_| err("query tracker lock"))?;
        let qt = match qt_guard.as_mut() {
            Some(q) => q,
            None => return Ok(false),
        };
        Ok(qt
            .track_query_completion(query, &project_path, std::path::Path::new(selected_file_path))
            .is_ok())
    }

    /// Historical query at `offset` (0 = most recent).
    fn get_historical_query(&self, offset: u64) -> PyResult<Option<String>> {
        self.ensure_alive()?;
        let project_path = {
            let pg = self
                .picker
                .read()
                .map_err(|e| err(format!("picker lock: {e}")))?;
            pg.as_ref().map(|p| p.base_path().to_path_buf())
        };
        let project_path = match project_path {
            Some(p) => p,
            None => return Ok(None),
        };
        let qt_guard = self
            .query_tracker
            .read()
            .map_err(|_| err("query tracker lock"))?;
        let qt = match qt_guard.as_ref() {
            Some(q) => q,
            None => return Ok(None),
        };
        Ok(qt
            .get_historical_query(&project_path, offset as usize)
            .ok()
            .flatten())
    }
}

impl FileFinder {
    fn ensure_alive(&self) -> PyResult<()> {
        if self.destroyed {
            return Err(err("FileFinder has been destroyed"));
        }
        Ok(())
    }
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("FffError", m.py().get_type_bound::<FffError>())?;
    m.add_class::<FileFinder>()?;
    m.add_class::<FileItem>()?;
    m.add_class::<Score>()?;
    m.add_class::<Location>()?;
    m.add_class::<SearchResult>()?;
    m.add_class::<DirItem>()?;
    m.add_class::<DirSearchResult>()?;
    m.add_class::<MixedItem>()?;
    m.add_class::<MixedSearchResult>()?;
    m.add_class::<GrepMatch>()?;
    m.add_class::<GrepResult>()?;
    m.add_class::<ScanProgress>()?;
    Ok(())
}
