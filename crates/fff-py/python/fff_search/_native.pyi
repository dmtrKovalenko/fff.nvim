from __future__ import annotations

from typing import Optional

__version__: str

class FffError(RuntimeError): ...

class FileItem:
    relative_path: str
    file_name: str
    git_status: str
    size: int
    modified: int
    access_frecency_score: int
    modification_frecency_score: int
    total_frecency_score: int
    is_binary: bool

class Score:
    total: int
    base_score: int
    filename_bonus: int
    special_filename_bonus: int
    frecency_boost: int
    distance_penalty: int
    current_file_penalty: int
    combo_match_boost: int
    path_alignment_bonus: int
    exact_match: bool
    match_type: str

class Location:
    kind: str
    line: int
    col: int
    end_line: int
    end_col: int

class SearchResult:
    items: list[FileItem]
    scores: list[Score]
    total_matched: int
    total_files: int
    location: Optional[Location]
    def __len__(self) -> int: ...

class DirItem:
    relative_path: str
    dir_name: str
    max_access_frecency: int

class DirSearchResult:
    items: list[DirItem]
    scores: list[Score]
    total_matched: int
    total_dirs: int
    def __len__(self) -> int: ...

class MixedItem:
    kind: str
    file: Optional[FileItem]
    directory: Optional[DirItem]

class MixedSearchResult:
    items: list[MixedItem]
    scores: list[Score]
    total_matched: int
    total_files: int
    total_dirs: int
    location: Optional[Location]
    def __len__(self) -> int: ...

class GrepMatch:
    relative_path: str
    file_name: str
    git_status: str
    line_content: str
    match_ranges: list[tuple[int, int]]
    context_before: list[str]
    context_after: list[str]
    size: int
    modified: int
    total_frecency_score: int
    access_frecency_score: int
    modification_frecency_score: int
    line_number: int
    byte_offset: int
    col: int
    fuzzy_score: Optional[int]
    is_binary: bool
    is_definition: bool

class GrepResult:
    items: list[GrepMatch]
    total_matched: int
    total_files_searched: int
    total_files: int
    filtered_file_count: int
    next_cursor: Optional[int]
    regex_fallback_error: Optional[str]
    def __len__(self) -> int: ...

class ScanProgress:
    scanned_files_count: int
    is_scanning: bool
    is_watcher_ready: bool
    is_warmup_complete: bool

class FileFinder:
    is_destroyed: bool

    @staticmethod
    def create(
        base_path: str,
        *,
        frecency_db_path: Optional[str] = None,
        history_db_path: Optional[str] = None,
        disable_mmap_cache: bool = False,
        disable_content_indexing: Optional[bool] = None,
        disable_watch: bool = False,
        ai_mode: bool = False,
        log_file_path: Optional[str] = None,
        log_level: Optional[str] = None,
        cache_budget_max_files: int = 0,
        cache_budget_max_bytes: int = 0,
        cache_budget_max_file_size: int = 0,
    ) -> FileFinder: ...
    def destroy(self) -> None: ...
    def __enter__(self) -> FileFinder: ...
    def __exit__(self, exc_type: object, exc_val: object, exc_tb: object) -> bool: ...
    def wait_for_scan(self, timeout_ms: int) -> bool: ...
    def wait_for_watcher(self, timeout_ms: int) -> bool: ...
    def is_scanning(self) -> bool: ...
    def get_scan_progress(self) -> ScanProgress: ...
    def scan_files(self) -> None: ...
    def refresh_git_status(self) -> int: ...
    def get_base_path(self) -> Optional[str]: ...
    def file_search(
        self,
        query: str,
        *,
        current_file: Optional[str] = None,
        max_threads: int = 0,
        page_index: int = 0,
        page_size: int = 0,
        combo_boost_multiplier: int = 0,
        min_combo_count: int = 0,
    ) -> SearchResult: ...
    def directory_search(
        self,
        query: str,
        *,
        current_file: Optional[str] = None,
        max_threads: int = 0,
        page_index: int = 0,
        page_size: int = 0,
    ) -> DirSearchResult: ...
    def mixed_search(
        self,
        query: str,
        *,
        current_file: Optional[str] = None,
        max_threads: int = 0,
        page_index: int = 0,
        page_size: int = 0,
        combo_boost_multiplier: int = 0,
        min_combo_count: int = 0,
    ) -> MixedSearchResult: ...
    def grep(
        self,
        query: str,
        *,
        mode: str = "plain",
        max_file_size: int = 0,
        max_matches_per_file: int = 0,
        smart_case: bool = True,
        cursor: Optional[int] = None,
        page_limit: int = 0,
        time_budget_ms: int = 0,
        before_context: int = 0,
        after_context: int = 0,
        classify_definitions: bool = False,
    ) -> GrepResult: ...
    def multi_grep(
        self,
        patterns: list[str],
        *,
        constraints: Optional[str] = None,
        max_file_size: int = 0,
        max_matches_per_file: int = 0,
        smart_case: bool = True,
        cursor: Optional[int] = None,
        page_limit: int = 0,
        time_budget_ms: int = 0,
        before_context: int = 0,
        after_context: int = 0,
        classify_definitions: bool = False,
    ) -> GrepResult: ...
    def track_query(self, query: str, selected_file_path: str) -> bool: ...
    def get_historical_query(self, offset: int) -> Optional[str]: ...
