#!/usr/bin/env python3
"""Force public `*Error` enums to implement `std::error::Error` cleanly.

A public type named `*Error` that does not implement `std::error::Error` is a
soft contract violation: downstream callers can no longer `?`-propagate it
through a typed Result chain without manual wrapping, which usually leads to
ad-hoc `Box<dyn Error>` or — worse — silent fallback (`.ok()`, `unwrap_or`).

Structural invariants:

  1. Every `pub enum *Error` (or `pub struct *Error`) in workspace source must
     satisfy ONE of:
       (a) a `#[derive(...)]` line within the preceding 6 lines includes
           `Error` (the thiserror derive form), OR
       (b) the same file contains `impl std::error::Error for X` AND an
           `impl <something>Display for X` (manual impl form — preferred in
           this repo because thiserror's proc-macro derive adds cold-build
           cost the build-hygiene policy wants to avoid).
  2. If form (a) is used, every variant of a `pub enum *Error` must carry its
     own `#[error("...")]` attribute. Form (b) hand-rolls Display, so
     per-variant attributes are not required.

Tests directories are excluded. Sealed / non-public error types are out of
scope; this is a public-surface gate.

Wire-protocol DTOs named `*Error` (carry-error-on-the-wire structs without
Rust-side Error semantics) are explicitly allowlisted via WIRE_DTO_ERRORS.
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

DERIVE_RE = re.compile(r"#\[\s*derive\s*\(([^)]+)\)\s*\]")
PUB_ERROR_DECL_RE = re.compile(
    r"^\s*pub\s+(?P<kind>enum|struct)\s+(?P<name>[A-Z][A-Za-z0-9_]*Error)\b"
)
ERROR_ATTR_RE = re.compile(r"#\[\s*error\s*\(")
VARIANT_RE = re.compile(r"^\s*(?P<name>[A-Z][A-Za-z0-9_]*)\s*(?:\{|\(|,|$)")

# Types named `*Error` that are NOT Rust error types — typically wire-protocol
# DTOs carrying error-shaped payloads across IPC. They do not need to implement
# std::error::Error because callers map them into a typed local Error first.
WIRE_DTO_ERRORS: frozenset[str] = frozenset(
    {
        # IPC envelope: `code` + `message` pair carried on the wire. Consumers
        # convert into a local Rust error before propagating.
        "SearchPlaneIpcError",
    }
)


def has_manual_error_impl(text: str, type_name: str) -> bool:
    """Return True iff `text` contains `impl std::error::Error for <type_name>`."""
    pattern = re.compile(
        rf"impl\s+(?:std::error::|core::error::)?Error\s+for\s+{re.escape(type_name)}\b"
    )
    return bool(pattern.search(text))


def has_display_impl(text: str, type_name: str) -> bool:
    """Return True iff `text` has any Display impl for `type_name`."""
    pattern = re.compile(
        rf"impl\s+(?:[\w:]+::)?(?:fmt::|std::fmt::|core::fmt::)?Display\s+for\s+{re.escape(type_name)}\b"
    )
    return bool(pattern.search(text))


@dataclass(frozen=True)
class Violation:
    path: Path
    line: int
    message: str


def workspace_members() -> list[Path]:
    data = tomllib.loads(WORKSPACE_TOML.read_text(encoding="utf-8"))
    return [ROOT / m for m in data.get("workspace", {}).get("members", [])]


def crate_source_files() -> list[Path]:
    files: list[Path] = []
    for member in workspace_members():
        src = member / "src"
        if not src.is_dir():
            continue
        for rs in sorted(src.rglob("*.rs")):
            if any(part == "tests" for part in rs.parts):
                continue
            files.append(rs)
    return files


def has_thiserror_derive(lines: list[str], decl_idx: int) -> bool:
    """Walk back up to 6 non-blank lines looking for a derive(... Error ...)."""
    seen = 0
    for offset in range(1, 10):
        idx = decl_idx - offset
        if idx < 0:
            break
        line = lines[idx].strip()
        if not line:
            continue
        seen += 1
        if seen > 6:
            break
        m = DERIVE_RE.search(line)
        if m:
            names = [n.strip().rsplit("::", 1)[-1] for n in m.group(1).split(",")]
            if "Error" in names:
                return True
    return False


def audit_enum_variants(
    path: Path, lines: list[str], decl_idx: int, enum_name: str
) -> list[Violation]:
    """Walk the enum body. Each variant declaration must carry #[error("...")]."""
    findings: list[Violation] = []
    depth = 0
    opened = False
    pending_error_attr = False

    for i in range(decl_idx, len(lines)):
        line = lines[i]
        stripped = line.strip()

        # Track {} balance to find body end.
        depth += line.count("{") - line.count("}")
        if line.count("{") > 0:
            opened = True
        if opened and depth <= 0:
            break

        # Skip the declaration line.
        if i == decl_idx:
            continue

        # Track attributes accumulating above a variant.
        if ERROR_ATTR_RE.search(stripped):
            pending_error_attr = True
            continue
        if stripped.startswith("#["):
            # Some other attribute — does not reset pending_error_attr.
            continue
        if not stripped or stripped.startswith("//"):
            continue
        if stripped.startswith("}"):
            continue

        # This should be a variant identifier line.
        m = VARIANT_RE.match(stripped)
        if not m:
            # Possibly continuation of a tuple/struct variant body — skip.
            continue

        if not pending_error_attr:
            findings.append(
                Violation(
                    path,
                    i + 1,
                    f"variant `{enum_name}::{m.group('name')}` lacks "
                    f"`#[error(\"...\")]` attribute",
                )
            )
        pending_error_attr = False

    return findings


def audit_file(path: Path) -> list[Violation]:
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    findings: list[Violation] = []
    for idx, line in enumerate(lines):
        m = PUB_ERROR_DECL_RE.match(line)
        if not m:
            continue
        name = m.group("name")
        kind = m.group("kind")

        if name in WIRE_DTO_ERRORS:
            # Wire-protocol DTO — opt-out by explicit allowlist.
            continue

        derived = has_thiserror_derive(lines, idx)
        manual_error = has_manual_error_impl(text, name)
        manual_display = has_display_impl(text, name)
        manual_ok = manual_error and manual_display

        if not derived and not manual_ok:
            findings.append(
                Violation(
                    path,
                    idx + 1,
                    f"`pub {kind} {name}` does not implement `std::error::Error`. "
                    "Either add `#[derive(Debug, thiserror::Error)]` above it "
                    "(thiserror form) OR add a manual `impl std::error::Error "
                    f"for {name}` AND `impl Display for {name}` block in the "
                    "same file (manual form).",
                )
            )
            continue

        if kind == "enum" and derived:
            # `#[error("...")]` per variant is only required when thiserror
            # is doing the Display work. Manual Display already covers it.
            findings.extend(audit_enum_variants(path, lines, idx, name))
    return findings


def main() -> int:
    violations: list[Violation] = []
    files = crate_source_files()
    for path in files:
        violations.extend(audit_file(path))

    if violations:
        print("Error-shape check failed:", file=sys.stderr)
        for v in violations:
            rel = v.path.relative_to(ROOT) if v.path.is_absolute() else v.path
            print(f"  - {rel}:{v.line}: {v.message}", file=sys.stderr)
        return 1

    print(f"All public *Error types in {len(files)} source files have shape OK.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
