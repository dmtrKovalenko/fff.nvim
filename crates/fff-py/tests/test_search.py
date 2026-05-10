from __future__ import annotations

from pathlib import Path

from fff_search import FileFinder


def _open(repo: Path, **kw) -> FileFinder:
    f = FileFinder.create(base_path=str(repo), disable_watch=True, **kw)
    assert f.wait_for_scan(timeout_ms=10_000)
    return f


def test_file_search_finds_main(sample_repo: Path):
    with _open(sample_repo) as f:
        r = f.file_search("main", page_size=20)
        assert r.total_matched >= 1
        paths = {item.relative_path for item in r.items}
        assert any(p.endswith("main.py") for p in paths)


def test_file_search_typo_resistant(sample_repo: Path):
    with _open(sample_repo) as f:
        # "raedme" should still find README.md via fuzzy matching
        r = f.file_search("raedme", page_size=10)
        assert any(item.file_name.lower().startswith("readme") for item in r.items)


def test_file_search_returns_score(sample_repo: Path):
    with _open(sample_repo) as f:
        r = f.file_search("main.py", page_size=5)
        assert len(r.scores) == len(r.items)
        if r.items:
            assert r.scores[0].total > 0


def test_directory_search(sample_repo: Path):
    with _open(sample_repo) as f:
        r = f.directory_search("src", page_size=5)
        names = {d.dir_name for d in r.items}
        assert any("src" in n for n in names)


def test_mixed_search_yields_files_and_dirs(sample_repo: Path):
    with _open(sample_repo) as f:
        r = f.mixed_search("src", page_size=20)
        kinds = {it.kind for it in r.items}
        assert "file" in kinds or "directory" in kinds


def test_search_pagination(sample_repo: Path):
    # page_index is a raw item offset (matches Node SDK)
    with _open(sample_repo) as f:
        first = f.file_search("py", page_index=0, page_size=2)
        second = f.file_search("py", page_index=2, page_size=2)
        if first.items and second.items:
            assert {i.relative_path for i in first.items}.isdisjoint(
                {i.relative_path for i in second.items}
            )


def test_search_total_matched_consistent(sample_repo: Path):
    with _open(sample_repo) as f:
        a = f.file_search("py", page_index=0, page_size=2)
        b = f.file_search("py", page_index=0, page_size=10)
        assert a.total_matched == b.total_matched
