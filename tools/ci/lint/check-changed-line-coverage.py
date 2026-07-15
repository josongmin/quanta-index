#!/usr/bin/env python3
"""Fail closed when changed production Rust lines lack LCOV execution evidence."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[3]
HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")


@dataclass(frozen=True)
class CoverageResult:
    covered_lines: int
    total_lines: int
    violations: list[str]

    @property
    def percent(self) -> float | None:
        if self.total_lines == 0:
            return None
        return (self.covered_lines * 100.0) / self.total_lines


def _relative_source_path(root: Path, raw_path: str) -> str | None:
    source = Path(raw_path)
    try:
        relative = source.resolve().relative_to(root.resolve())
    except ValueError:
        return None
    return relative.as_posix()


def parse_lcov(root: Path, lcov_path: Path) -> dict[str, dict[int, int]]:
    """Read line hit counts keyed by repository-relative source path."""
    if not lcov_path.is_file():
        raise ValueError(f"LCOV artifact does not exist: {lcov_path}")

    coverage: dict[str, dict[int, int]] = {}
    current_source: str | None = None
    for raw_line in lcov_path.read_text(encoding="utf-8").splitlines():
        if raw_line.startswith("SF:"):
            current_source = _relative_source_path(root, raw_line.removeprefix("SF:"))
            continue
        if not raw_line.startswith("DA:") or current_source is None:
            continue
        payload = raw_line.removeprefix("DA:")
        try:
            line_text, hits_text, *_ = payload.split(",")
            line_number = int(line_text)
            hits = int(hits_text)
        except ValueError as error:
            raise ValueError(f"invalid LCOV DA record {raw_line!r}: {error}") from error
        if line_number <= 0 or hits < 0:
            raise ValueError(f"invalid LCOV DA record {raw_line!r}")
        coverage.setdefault(current_source, {})[line_number] = hits
    return coverage


def _is_production_rust(path: str) -> bool:
    candidate = PurePosixPath(path)
    return (
        len(candidate.parts) >= 4
        and candidate.parts[0] == "crates"
        and candidate.parts[2] == "src"
        and candidate.suffix == ".rs"
    )


def changed_production_lines(root: Path, base: str) -> set[tuple[str, int]]:
    """Return added production Rust lines in ``base...HEAD`` without inference."""
    result = subprocess.run(
        ["git", "diff", "--no-ext-diff", "--unified=0", f"{base}...HEAD", "--", "crates"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise ValueError(
            f"cannot diff coverage base {base!r}: {result.stderr.strip() or result.stdout.strip()}"
        )

    changed: set[tuple[str, int]] = set()
    current_path: str | None = None
    for line in result.stdout.splitlines():
        if line.startswith("+++ b/"):
            candidate = line.removeprefix("+++ b/")
            current_path = candidate if _is_production_rust(candidate) else None
            continue
        match = HUNK_RE.match(line)
        if match is None or current_path is None:
            continue
        start = int(match.group(1))
        count = int(match.group(2) or "1")
        changed.update((current_path, line_number) for line_number in range(start, start + count))
    return changed


def audit_changed_coverage(
    root: Path,
    lcov_path: Path,
    changed_lines: set[tuple[str, int]],
    minimum_percent: float,
) -> CoverageResult:
    """Check changed lines against LCOV; absent evidence is a violation."""
    if not 0.0 <= minimum_percent <= 100.0:
        raise ValueError("minimum percent must be within 0..=100")

    coverage = parse_lcov(root, lcov_path)
    violations: list[str] = []
    covered = 0
    for path, line_number in sorted(changed_lines):
        file_coverage = coverage.get(path)
        if file_coverage is None:
            violations.append(f"changed source missing from LCOV: {path}")
            continue
        hits = file_coverage.get(line_number)
        if hits is None:
            violations.append(f"changed line missing from LCOV: {path}:{line_number}")
            continue
        if hits == 0:
            violations.append(f"uncovered changed line: {path}:{line_number}")
            continue
        covered += 1

    total = len(changed_lines)
    result = CoverageResult(covered_lines=covered, total_lines=total, violations=violations)
    if result.percent is not None and result.percent < minimum_percent:
        violations.append(
            f"changed-line coverage {result.percent:.2f}% is below required {minimum_percent:.2f}%"
        )
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lcov", type=Path, required=True, help="LCOV artifact produced for HEAD")
    parser.add_argument("--base", required=True, help="Git revision used as the changed-line base")
    parser.add_argument("--minimum-percent", type=float, default=90.0)
    args = parser.parse_args()

    try:
        changed = changed_production_lines(ROOT, args.base)
        result = audit_changed_coverage(ROOT, args.lcov, changed, args.minimum_percent)
    except ValueError as error:
        print(f"changed-line coverage gate error: {error}", file=sys.stderr)
        return 2

    if result.percent is None:
        print("changed-line coverage: no changed production Rust lines; no coverage claim made")
        return 0

    print(
        "changed-line coverage: "
        f"{result.covered_lines}/{result.total_lines} ({result.percent:.2f}%), "
        f"required {args.minimum_percent:.2f}%"
    )
    for violation in result.violations:
        print(f"coverage violation: {violation}", file=sys.stderr)
    return 1 if result.violations else 0


if __name__ == "__main__":
    raise SystemExit(main())
