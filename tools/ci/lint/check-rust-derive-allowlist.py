#!/usr/bin/env python3
"""Allowlist-based guard for `#[derive(...)]` invocations.

This script is the sole derive policy: it rejects the known-bad serde derives
and allows only *cheap* derives. Any future proc-macro derive (`strum::EnumIter`,
`clap::Parser`, `Deserialize_repr`, `tokio::main`, ...) silently slipping into
an owned Rust source file is treated as build-cost regression and fails the gate.

The allowlist is intentionally small. Adding a new entry requires:
  1. measuring its cost via `cargo llvm-lines` / `cargo --timings`
  2. updating the allowlist AND the rule catalog source under
     `tools/prompt-manager/sources/rules/catalog.md`
  3. re-running `python3 tools/prompt-manager/pm.py sync`
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
RUST_SOURCE_ROOTS = (ROOT / "crates", ROOT / "benchmarks")

ALLOWED_DERIVES: frozenset[str] = frozenset(
    {
        "Clone",
        "Copy",
        "Debug",
        "Default",
        "Eq",
        "Hash",
        "Ord",
        "PartialEq",
        "PartialOrd",
        # thiserror::Error is the only `Error` derive in tree. cargo-deny +
        # cargo-machete catch any new proc-macro crate, so an unfamiliar
        # `Error` derive cannot land silently.
        "Error",
    }
)

# Banned outright within the single owned derive policy.
EXPLICIT_BAN: dict[str, str] = {
    "Serialize": "manual `impl serde::Serialize` is required",
    "Deserialize": "manual `impl serde::Deserialize` is required",
    "Serialize_repr": "use manual repr-tagged enum serialization",
    "Deserialize_repr": "use manual repr-tagged enum deserialization",
}


def last_segment(name: str) -> str:
    """Strip `serde::` / `thiserror::` / etc., return the trailing identifier."""
    return name.rsplit("::", 1)[-1]


def derives_in(text: str) -> list[tuple[int, list[str]]]:
    """Return (line_number, [derive_name, ...]) for every derive site in text."""
    from tree_sitter_language_pack import get_parser

    sites: list[tuple[int, list[str]]] = []
    tree = get_parser("rust").parse(text.encode("utf-8"))

    # Other source syntax errors are owned by rustc. An invalid derive site must
    # still be rejected here because the parser may otherwise omit it.
    def invalid_derive(node: object) -> bool:
        if node.type == "ERROR" and b"derive" in node.text:
            return True
        return any(invalid_derive(child) for child in node.children)

    if invalid_derive(tree.root_node):
        raise ValueError("invalid derive syntax; derive policy cannot certify it")

    def visit(node: object) -> None:
        if node.type in {"attribute_item", "inner_attribute_item"}:
            stack = [node]
            while stack:
                current = stack.pop()
                children = current.children
                for index, child in enumerate(children):
                    if child.type == "identifier" and child.text == b"derive":
                        argument = children[index + 1] if index + 1 < len(children) else None
                        if argument is None:
                            raise ValueError("derive attribute has no argument list")
                        if argument.type != "token_tree":
                            raise ValueError("derive attribute has no argument list")
                        body = argument.text.decode("utf-8")[1:-1]
                        names = [name.strip() for name in body.split(",") if name.strip()]
                        sites.append((child.start_point.row + 1, names))
                    stack.append(child)
            return
        for child in node.children:
            visit(child)

    visit(tree.root_node)
    return sites


def audit_file(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    findings: list[str] = []
    for line, names in derives_in(text):
        for raw in names:
            name = last_segment(raw)
            if name in EXPLICIT_BAN:
                findings.append(
                    f"{path}:{line}: derive `{raw}` is banned outright ({EXPLICIT_BAN[name]})"
                )
                continue
            if name not in ALLOWED_DERIVES:
                findings.append(
                    f"{path}:{line}: derive `{raw}` is not on the allowlist. "
                    f"Cheap derives only: {sorted(ALLOWED_DERIVES)}. "
                    f"To add one, measure cost (cargo llvm-lines / --timings) "
                    f"and update tools/ci/lint/check-rust-derive-allowlist.py "
                    f"plus tools/prompt-manager/sources/rules/catalog.md."
                )
    return findings


def rust_source_files(roots: tuple[Path, ...] = RUST_SOURCE_ROOTS) -> list[Path]:
    """Return every owned Rust source under the guarded roots."""
    return sorted(
        path
        for source_root in roots
        for path in source_root.rglob("*.rs")
        if "/target/" not in path.as_posix()
    )


def main() -> int:
    missing = [path for path in RUST_SOURCE_ROOTS if not path.is_dir()]
    if missing:
        print(f"Rust source root(s) not found: {missing}", file=sys.stderr)
        return 2

    findings: list[str] = []
    for rs in rust_source_files():
        findings.extend(audit_file(rs))

    if findings:
        for line in findings:
            print(line, file=sys.stderr)
        print(
            f"\n{len(findings)} disallowed derive(s) found. "
            "Update the allowlist or rewrite the derive as a manual impl.",
            file=sys.stderr,
        )
        return 1

    print("All `#[derive(...)]` sites are on the allowlist.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
