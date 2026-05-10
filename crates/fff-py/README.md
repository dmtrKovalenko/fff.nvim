# fff-search

Python bindings for [fff](https://github.com/dmtrKovalenko/fff.nvim).

```bash
pip install fff-search
```

```python
from fff_search import FileFinder

with FileFinder.create(base_path=".", ai_mode=True) as f:
    f.wait_for_scan(timeout_ms=10_000)
    files = f.file_search("incognito profile", page_size=20)
    hits = f.grep("GetOffTheRecordProfile", classify_definitions=True)
```

Errors raise `FffError`. Type stubs bundled.

Wheels: Linux (x86_64, aarch64; manylinux + musllinux), macOS (x86_64, aarch64), Windows (x86_64). Python 3.9+ via abi3.

## API

See [`python/fff_search/_native.pyi`](python/fff_search/_native.pyi). Methods on `FileFinder`:

- `file_search`, `directory_search`, `mixed_search`
- `grep(query, mode='plain'|'regex'|'fuzzy', ...)`, `multi_grep(patterns, ...)`
- `scan_files`, `refresh_git_status`
- `wait_for_scan`, `wait_for_watcher`, `is_scanning`, `get_scan_progress`
- `track_query`, `get_historical_query`

## Develop

```bash
pip install maturin pytest
cd crates/fff-py
maturin develop --release
pytest tests/
```
