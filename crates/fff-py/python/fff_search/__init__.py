"""Python bindings for fff. See https://github.com/dmtrKovalenko/fff.nvim."""

from ._native import (
    DirItem,
    DirSearchResult,
    FffError,
    FileFinder,
    FileItem,
    GrepMatch,
    GrepResult,
    Location,
    MixedItem,
    MixedSearchResult,
    ScanProgress,
    Score,
    SearchResult,
    __version__,
)

__all__ = [
    "FileFinder",
    "FileItem",
    "Score",
    "Location",
    "SearchResult",
    "DirItem",
    "DirSearchResult",
    "MixedItem",
    "MixedSearchResult",
    "GrepMatch",
    "GrepResult",
    "ScanProgress",
    "FffError",
    "__version__",
]
