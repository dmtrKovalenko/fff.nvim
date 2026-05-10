from __future__ import annotations

import os
import sys
import tempfile

from fff_search import FileFinder


def main() -> int:
    base = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else ".")

    with tempfile.TemporaryDirectory() as tmp:
        with FileFinder.create(
            base_path=base,
            frecency_db_path=os.path.join(tmp, "frecency"),
            history_db_path=os.path.join(tmp, "history"),
            ai_mode=True,
        ) as finder:
            print(f"Indexing {base!r}…")
            ok = finder.wait_for_scan(timeout_ms=10_000)
            if not ok:
                print("Scan timed out", file=sys.stderr)
                return 1

            print("\n== fuzzy file_search('readme') ==")
            r = finder.file_search("readme", page_size=10)
            print(r)
            for item, score in zip(r.items, r.scores):
                print(f"  {score.total:>6}  {item.relative_path}")

            print("\n== grep('TODO', mode='plain') ==")
            g = finder.grep("TODO", mode="plain", page_limit=10)
            print(g)
            for m in g.items[:10]:
                print(f"  {m.relative_path}:{m.line_number}  {m.line_content[:80]}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
