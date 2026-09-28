#!/usr/bin/env python3
from __future__ import annotations

import re
import sys
from functools import cache
from pathlib import Path
from urllib.parse import unquote

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
HEADING_RE = re.compile(r"^#{1,6}\s+(.+?)\s*#*\s*$")


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


@cache
def heading_anchors(path: Path) -> frozenset[str]:
    anchors: set[str] = set()
    duplicates: dict[str, int] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        match = HEADING_RE.match(line)
        if match is None:
            continue
        heading = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", match.group(1))
        heading = re.sub(r"<[^>]+>", "", heading).replace("`", "")
        slug = re.sub(r"[^\w\- ]", "", heading.lower()).replace(" ", "-")
        count = duplicates.get(slug, 0)
        anchors.add(f"{slug}-{count}" if count else slug)
        duplicates[slug] = count + 1
    return frozenset(anchors)


def link_issue(doc_path: Path, raw_target: str, repo_root: Path = REPO_ROOT) -> str | None:
    target = raw_target.strip()
    if not target or "://" in target or target.startswith("mailto:"):
        return None
    normalized, _, fragment = target.partition("#")
    candidate = (doc_path.parent / normalized).resolve() if normalized else doc_path.resolve()
    try:
        candidate.relative_to(repo_root)
    except ValueError:
        return "broken doc path"
    if not candidate.exists():
        return "broken doc path"
    if fragment and candidate.suffix in DOC_SUFFIXES:
        if unquote(fragment) not in heading_anchors(candidate):
            return "broken doc anchor"
    return None


def main() -> int:
    errors: list[str] = []
    for doc_path in iter_docs():
        content = doc_path.read_text(encoding="utf-8")
        rel = doc_path.relative_to(REPO_ROOT)
        for line_no, line in enumerate(content.splitlines(), start=1):
            for match in LINK_RE.finditer(line):
                target = match.group(1)
                issue = link_issue(doc_path, target)
                if issue:
                    errors.append(f"{rel}:{line_no}: {issue}: {target}")
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        print(f"\nfound {len(errors)} broken doc link(s)", file=sys.stderr)
        return 1
    print("all doc paths and anchors resolved", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
