#!/usr/bin/env python3
"""Force `mod.rs` files and `quanta-index-contract/src/lib.rs` to be re-export
facades only — no in-line implementation items.

Rationale: when implementation slips into a mod.rs file, three things break:
  1. The module's surface becomes uneven — half the items live in named files,
     half live in the mod.rs prelude. Code review can no longer answer
     "what does this module export" from a single file read.
  2. Renames cascade poorly: moving a `pub fn` out of mod.rs forces every call
     site to update at the same time the mod tree shifts.
  3. The contract crate (DTO-only) loses its DTO-only invariant the moment a
     `fn` body shows up in lib.rs — and the breakage is only catchable by
     reading every file rather than by gating the entrypoint.

Rules:

  * Every `mod.rs` workspace-wide may contain ONLY: attributes (`#![...]` /
    `#[...]`), doc-comments / line comments, `use`, `pub use`, `mod`,
    `pub mod`, `pub(crate) mod`, `pub(super) mod`, blank lines.
  * `crates/quanta-index-contract/src/lib.rs` obeys the same restriction.

Disallowed at the top level of these files:
  * `fn`, `pub fn`, `pub(crate) fn`
  * `struct`, `pub struct`, `enum`, `pub enum`
  * `trait`, `pub trait`, `impl`
  * `const`, `pub const`, `static`, `pub static`
  * `macro_rules!`
  * `extern`
  * inline `mod foo { ... }` block bodies (must be file-backed)

Other crates' `lib.rs` are exempt — they may legitimately host composition
helpers or factories.
"""

from __future__ import annotations

import sys
from dataclasses import dataclass
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
WORKSPACE_TOML = ROOT / "Cargo.toml"
CRATES_DIR = ROOT / "crates"
CONTRACT_LIB_RS = CRATES_DIR / "quanta-index-contract" / "src" / "lib.rs"


@dataclass(frozen=True)
class Violation:
    path: Path
    line: int
    snippet: str


def workspace_members() -> list[Path]:
    data = tomllib.loads(WORKSPACE_TOML.read_text(encoding="utf-8"))
    return [ROOT / m for m in data.get("workspace", {}).get("members", [])]


def collect_facade_files() -> list[Path]:
    """Return every `mod.rs` under workspace members, plus the contract lib.rs."""
    files: list[Path] = []
    for member in workspace_members():
        src = member / "src"
        if src.is_dir():
            files.extend(sorted(src.rglob("mod.rs")))
    if CONTRACT_LIB_RS.exists():
        files.append(CONTRACT_LIB_RS)
    return files


def audit_facade(path: Path) -> list[Violation]:
    from tree_sitter_language_pack import get_parser

    source = path.read_bytes()
    tree = get_parser("rust").parse(source)
    findings: list[Violation] = []
    for node in tree.root_node.children:
        if node.type in {
            "line_comment",
            "block_comment",
            "attribute_item",
            "inner_attribute_item",
            "use_declaration",
        }:
            continue
        if node.type == "mod_item" and any(child.type == ";" for child in node.children):
            continue
        snippet = source[node.start_byte : node.end_byte].decode("utf-8", errors="replace")
        findings.append(Violation(path, node.start_point.row + 1, snippet.splitlines()[0][:120]))
    if tree.root_node.has_error and not findings:
        findings.append(Violation(path, 1, "invalid Rust syntax in facade"))
    return findings


def main() -> int:
    violations: list[Violation] = []
    for path in collect_facade_files():
        violations.extend(audit_facade(path))

    if violations:
        print("Module discipline check failed:", file=sys.stderr)
        for v in violations:
            rel = v.path.relative_to(ROOT) if v.path.is_absolute() else v.path
            print(
                f"  - {rel}:{v.line}: facade file must contain only "
                f"use/pub use/mod declarations and attributes, found: {v.snippet!r}",
                file=sys.stderr,
            )
        print(
            "\nFix: move the implementation to a named sibling file and add a "
            "`pub mod <name>;` / `pub use <name>::*;` re-export here instead.",
            file=sys.stderr,
        )
        return 1

    print(f"All {len(collect_facade_files())} facade files pass module discipline.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
