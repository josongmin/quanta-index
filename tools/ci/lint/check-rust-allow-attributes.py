#!/usr/bin/env python3
"""Reject literal allow attributes, including formatted and conditional sites."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from rust_attribute_policy import attribute_metas
from tree_sitter_language_pack import get_parser

ROOT = Path(__file__).resolve().parents[3]


def allow_sites(text: str) -> list[int]:
    tree = get_parser("rust").parse(text.encode("utf-8"))
    sites: list[int] = []

    def visit(node: object) -> None:
        if node.type in {"attribute_item", "inner_attribute_item"}:
            if any(name == "allow" for name, _, _ in attribute_metas(node)):
                sites.append(node.start_point.row + 1)
            return
        if node.type == "ERROR" and b"#" in node.text:
            raise ValueError("invalid Rust attribute syntax")
        for child in node.children:
            visit(child)

    visit(tree.root_node)
    return sites


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    root = parser.parse_args(argv).root
    roots = (root / "crates", root / "benchmarks")
    if any(not path.is_dir() for path in roots):
        print("Rust source roots are missing", file=sys.stderr)
        return 2
    findings: list[str] = []
    try:
        for path in sorted(
            file
            for base in roots
            for file in base.rglob("*.rs")
            if "/target/" not in file.as_posix()
        ):
            for line in allow_sites(path.read_text(encoding="utf-8")):
                findings.append(f"{path}:{line}: allow attribute is banned; use an owned expect")
    except (OSError, ValueError) as exc:
        print(f"Rust allow scan failed: {exc}", file=sys.stderr)
        return 2
    if findings:
        print("\n".join(findings), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
