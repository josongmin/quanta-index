#!/usr/bin/env python3
from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
DOC_SUFFIXES = {".md", ".mdc"}
IGNORE_DIRS = {
    ".git",
    ".claude",
    "target",
    "state",
    ".venv",
    ".pytest_cache",
    ".ruff_cache",
    "sources",
}
LINK_RE = re.compile(r"\[[^\]]+\]\(([^)]+)\)")


def is_ignored_doc(path: Path) -> bool:
    rel = path.relative_to(REPO_ROOT)
    rel_parts = set(rel.parts)
    return bool(rel_parts & IGNORE_DIRS)


def iter_docs() -> list[Path]:
    docs: list[Path] = []
    for path in REPO_ROOT.rglob("*"):
        if not path.is_file():
            continue
        if path.suffix not in DOC_SUFFIXES:
            continue
        if is_ignored_doc(path):
            continue
        docs.append(path)
    return docs


def path_exists(doc_path: Path, raw_target: str) -> bool:
    target = raw_target.strip()
    if not target or target.startswith("#") or "://" in target or target.startswith("mailto:"):
        return True
    normalized = target.split("#", 1)[0]
    candidate = (doc_path.parent / normalized).resolve()
    try:
        candidate.relative_to(REPO_ROOT)
    except ValueError:
        return False
    return candidate.exists()


def main() -> int:
    errors: list[str] = []
    for doc_path in iter_docs():
        content = doc_path.read_text(encoding="utf-8")
        rel = doc_path.relative_to(REPO_ROOT)
        for line_no, line in enumerate(content.splitlines(), start=1):
            for match in LINK_RE.finditer(line):
                target = match.group(1)
                if not path_exists(doc_path, target):
                    errors.append(f"{rel}:{line_no}: broken doc path: {target}")
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        print(f"\nfound {len(errors)} broken doc path(s)", file=sys.stderr)
        return 1
    print("all doc paths resolved", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
