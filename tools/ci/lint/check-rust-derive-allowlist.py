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

LINT_DIR = str(Path(__file__).resolve().parent)
if LINT_DIR not in sys.path:
    sys.path.insert(0, LINT_DIR)
from rust_attribute_policy import (  # noqa: E402
    attribute_metas,
    derive_names,
    macro_attribute_metas,
)
from rust_attribute_policy import (  # noqa: E402
    rust_source_files as inventory_sources,
)

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
        if node.type in {"macro_invocation", "macro_definition"}:
            for line, metas in macro_attribute_metas(node):
                for name, arguments, _ in metas:
                    if name == "derive":
                        sites.append((line, derive_names(arguments)))
            return
        if node.type in {"attribute_item", "inner_attribute_item"}:
            for name, arguments, _ in attribute_metas(node):
                if name != "derive":
                    continue
                sites.append((node.start_point.row + 1, derive_names(arguments)))
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
    return inventory_sources(roots)


def main() -> int:
    missing = [path for path in RUST_SOURCE_ROOTS if not path.is_dir()]
    if missing:
        print(f"Rust source root(s) not found: {missing}", file=sys.stderr)
        return 2

    findings: list[str] = []
    try:
        for rs in rust_source_files():
            findings.extend(audit_file(rs))
    except (OSError, RuntimeError, ValueError) as error:
        print(f"Rust derive scan failed: {error}", file=sys.stderr)
        return 2

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
