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

import re
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


# A line is "facade-acceptable" iff it matches one of these patterns when
# leading whitespace is stripped.
FACADE_PATTERNS: list[re.Pattern[str]] = [
    re.compile(r"^\s*$"),  # blank
    re.compile(r"^\s*//"),  # line comment / doc
    re.compile(r"^\s*/\*"),  # block comment open
    re.compile(r"^\s*\*"),  # block-comment continuation
    re.compile(r"^\s*\*/"),  # block comment close
    re.compile(r"^\s*#!?\["),  # attribute / inner attribute
    re.compile(r"^\s*use\b"),  # use ...;
    re.compile(r"^\s*pub\s+use\b"),  # pub use ...;
    re.compile(r"^\s*pub\([^)]*\)\s+use\b"),  # pub(crate) use / pub(super) use
    re.compile(r"^\s*mod\s+[A-Za-z_][\w]*\s*;"),  # mod foo;
    re.compile(r"^\s*pub\s+mod\s+[A-Za-z_][\w]*\s*;"),  # pub mod foo;
    re.compile(r"^\s*pub\s*\([^)]*\)\s+mod\s+[A-Za-z_][\w]*\s*;"),  # pub(crate) mod foo;
    re.compile(r"^\s*\}"),  # closing brace from a use-tree
    re.compile(r"^\s*[A-Za-z_][\w:]*\s*[,{]"),  # use-tree continuation
]


# Forbidden top-level item keywords — they indicate inline implementation.
FORBIDDEN_PREFIXES: list[re.Pattern[str]] = [
    re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?fn\s+"),
    re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+"),
    re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+"),
    re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?trait\s+"),
    re.compile(r"^\s*(?:unsafe\s+)?impl\b"),
    re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?const\s+"),
    re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?static\s+"),
    re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?type\s+"),
    re.compile(r"^\s*macro_rules!\s*"),
    re.compile(r"^\s*extern\s+"),
    re.compile(r"^\s*mod\s+[A-Za-z_][\w]*\s*\{"),  # inline mod foo {
    re.compile(r"^\s*pub\s+mod\s+[A-Za-z_][\w]*\s*\{"),
]


def is_inside_use_tree(text_so_far: str) -> bool:
    """Return True iff the cursor sits inside an unterminated `use { ... }` tree.

    A naive line-by-line lint would flag the inner identifiers of a multi-line
    `pub use channel::{\n    Foo,\n    Bar,\n};` block. Track brace depth to
    suppress those false positives.
    """
    open_braces = text_so_far.count("{") - text_so_far.count("}")
    return open_braces > 0


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
    text = path.read_text(encoding="utf-8")
    findings: list[Violation] = []
    # Track running text to detect "inside an open brace from a use-tree".
    running = ""
    for lineno, raw_line in enumerate(text.splitlines(), start=1):
        running_before = running
        running += raw_line + "\n"

        stripped = raw_line.rstrip()
        if not stripped:
            continue

        # If we're inside an open use-tree from a previous line, accept identifier
        # continuations and the closing brace.
        if is_inside_use_tree(running_before):
            continue

        # Allow attribute / comment / facade tokens.
        if any(p.match(stripped) for p in FACADE_PATTERNS):
            continue

        # If a forbidden keyword pattern matches, record it.
        if any(p.match(stripped) for p in FORBIDDEN_PREFIXES):
            findings.append(Violation(path, lineno, stripped[:120]))
            continue

        # Anything else at the top level is unexpected for a facade.
        findings.append(Violation(path, lineno, stripped[:120]))

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
