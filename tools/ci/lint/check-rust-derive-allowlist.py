#!/usr/bin/env python3
"""Allowlist-based guard for `#[derive(...)]` invocations.

Existing semgrep rule `rust-no-serde-derive` only blocks the two known-bad
derive names (Serialize, Deserialize). This script flips that to an explicit
allowlist of *cheap* derives. Any future proc-macro derive (`strum::EnumIter`,
`clap::Parser`, `Deserialize_repr`, `tokio::main`, ...) silently slipping into
a crate file is treated as build-cost regression and fails the gate.

The allowlist is intentionally small. Adding a new entry requires:
  1. measuring its cost via `cargo llvm-lines` / `cargo --timings`
  2. updating the allowlist AND the rule catalog source under
     `tools/prompt-manager/sources/rules/catalog.md`
  3. re-running `python3 tools/prompt-manager/pm.py sync`
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
CRATES_DIR = ROOT / "crates"

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

# Matches `#[derive(...)]` allowing nested parens via greedy capture across
# whitespace. We capture the comma-separated identifier list inside the parens.
DERIVE_RE = re.compile(r"#\[\s*derive\s*\(([^)]+)\)\s*\]", re.MULTILINE)

# Banned outright. These exist in clippy/semgrep already, but reasserting here
# means a single tool can audit the entire derive surface.
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
    sites: list[tuple[int, list[str]]] = []
    for match in DERIVE_RE.finditer(text):
        names = [token.strip() for token in match.group(1).split(",")]
        names = [n for n in names if n]
        line = text[: match.start()].count("\n") + 1
        sites.append((line, names))
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


def main() -> int:
    if not CRATES_DIR.exists():
        print(f"crates dir not found: {CRATES_DIR}", file=sys.stderr)
        return 2

    findings: list[str] = []
    for rs in sorted(CRATES_DIR.rglob("*.rs")):
        # Skip vendored/generated artifacts (none today; defensive).
        if "/target/" in str(rs):
            continue
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
