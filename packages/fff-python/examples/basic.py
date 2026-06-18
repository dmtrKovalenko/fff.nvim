"""Standalone example of using fff Python bindings."""

from __future__ import annotations

import sys
import time

from fff import FileFinder


def main() -> int:
    base_path = sys.argv[1] if len(sys.argv) > 1 else "."

    print(f"Indexing {base_path}...")
    start = time.time()
    with FileFinder(base_path, watch=False) as finder:
        print(f"Created in {time.time() - start:.2f}s")

        print("Waiting for scan...")
        finder.wait_for_scan_blocking(timeout_ms=30000)
        progress = finder.scan_progress
        print(f"Indexed {progress.scanned_files_count} files")

        print("\nFuzzy file search for 'main':")
        result = finder.search("main", page_size=5)
        for item, score in zip(result.items, result.scores):
            print(f"  {item.relative_path:<50} score={score.total}")

        print("\nGlob search '*.py':")
        result = finder.glob("*.py", page_size=5)
        for item in result.items:
            print(f"  {item.relative_path}")

        print("\nGrep for 'def ':")
        result = finder.grep("def ", page_limit=5)
        for match in result.items:
            print(f"  {match.relative_path}:{match.line_number}: {match.line_content.strip()}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
