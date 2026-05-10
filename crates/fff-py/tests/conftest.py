from __future__ import annotations

import shutil
import tempfile
from pathlib import Path
from typing import Iterator

import pytest


@pytest.fixture
def sample_repo() -> Iterator[Path]:
    tmp = Path(tempfile.mkdtemp(prefix="fff_py_"))
    try:
        (tmp / "src").mkdir()
        (tmp / "src" / "main.py").write_text(
            "def hello():\n    return 'world'\n\n"
            "class Greeter:\n    def greet(self):\n        return hello()\n",
            encoding="utf-8",
        )
        (tmp / "src" / "lib.py").write_text(
            "TODO: write the library\n"
            "def add(a, b):\n    return a + b\n",
            encoding="utf-8",
        )
        (tmp / "README.md").write_text(
            "# Sample Repo\n\nFor fff_search tests.\n", encoding="utf-8"
        )
        (tmp / "tests").mkdir()
        (tmp / "tests" / "test_basic.py").write_text(
            "def test_passes():\n    assert True\n", encoding="utf-8"
        )
        (tmp / "binary.dat").write_bytes(bytes([0, 1, 2, 3, 0xFF, 0xFE]))
        yield tmp
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
