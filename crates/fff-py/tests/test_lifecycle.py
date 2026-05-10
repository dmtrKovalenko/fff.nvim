from __future__ import annotations

from pathlib import Path

import pytest

from fff_search import FileFinder, FffError


def test_create_requires_base_path():
    with pytest.raises(FffError):
        FileFinder.create(base_path="")


def test_create_and_destroy(sample_repo: Path):
    finder = FileFinder.create(base_path=str(sample_repo), ai_mode=False, disable_watch=True)
    try:
        assert finder.is_destroyed is False
        assert finder.get_base_path() is not None
    finally:
        finder.destroy()
    assert finder.is_destroyed is True
    finder.destroy()


def test_context_manager(sample_repo: Path):
    with FileFinder.create(base_path=str(sample_repo), disable_watch=True) as f:
        assert f.is_destroyed is False
    assert f.is_destroyed is True


def test_use_after_destroy_raises(sample_repo: Path):
    f = FileFinder.create(base_path=str(sample_repo), disable_watch=True)
    f.destroy()
    with pytest.raises(FffError):
        f.file_search("anything")


def test_wait_for_scan_then_progress(sample_repo: Path):
    with FileFinder.create(base_path=str(sample_repo), disable_watch=True) as f:
        completed = f.wait_for_scan(timeout_ms=10_000)
        assert completed is True
        progress = f.get_scan_progress()
        assert progress.is_scanning is False
        assert progress.scanned_files_count >= 4
