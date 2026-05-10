from __future__ import annotations

from pathlib import Path

from fff_search import FileFinder


def _open(repo: Path) -> FileFinder:
    f = FileFinder.create(base_path=str(repo), disable_watch=True)
    assert f.wait_for_scan(timeout_ms=10_000)
    return f


def test_plain_grep_finds_todo(sample_repo: Path):
    with _open(sample_repo) as f:
        r = f.grep("TODO", mode="plain")
        assert any("TODO" in m.line_content for m in r.items)
        for m in r.items:
            assert m.line_number >= 1


def test_grep_classify_definitions(sample_repo: Path):
    with _open(sample_repo) as f:
        r = f.grep("def", mode="plain", classify_definitions=True, page_limit=20)
        assert any(m.is_definition for m in r.items)


def test_regex_grep(sample_repo: Path):
    with _open(sample_repo) as f:
        r = f.grep(r"def\s+\w+", mode="regex", page_limit=20)
        assert len(r.items) >= 1


def test_fuzzy_grep(sample_repo: Path):
    with _open(sample_repo) as f:
        r = f.grep("hllo", mode="fuzzy", page_limit=20)
        assert r.total_files_searched >= 1


def test_multi_grep(sample_repo: Path):
    with _open(sample_repo) as f:
        r = f.multi_grep(["TODO", "hello"])
        assert len(r.items) >= 1
        contents = {m.line_content for m in r.items}
        assert any("TODO" in c or "hello" in c for c in contents)


def test_grep_with_context(sample_repo: Path):
    with _open(sample_repo) as f:
        r = f.grep("TODO", mode="plain", before_context=1, after_context=1)
        if r.items:
            assert isinstance(r.items[0].context_before, list)
            assert isinstance(r.items[0].context_after, list)
