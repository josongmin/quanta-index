"""Contract tests for the fail-closed changed-line coverage gate."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-changed-line-coverage.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_changed_line_coverage", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_changed_line_coverage"] = module
    spec.loader.exec_module(module)
    return module


def _write_lcov(path: Path, source: Path, hits: dict[int, int]) -> Path:
    lines = [f"SF:{source}"]
    lines.extend(f"DA:{line},{count}" for line, count in sorted(hits.items()))
    lines.append("end_of_record")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


def test_changed_executable_lines_meeting_threshold_are_green(tmp_path: Path):
    module = _load_module()
    source = tmp_path / "crates" / "demo" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn demo() {}\n", encoding="utf-8")
    lcov = _write_lcov(tmp_path / "coverage.lcov", source, {1: 3, 2: 1})

    result = module.audit_changed_coverage(
        tmp_path,
        lcov,
        {
            (source.relative_to(tmp_path).as_posix(), 1),
            (source.relative_to(tmp_path).as_posix(), 2),
        },
        90.0,
    )

    assert result.violations == []
    assert result.covered_lines == 2
    assert result.total_lines == 2


def test_uncovered_changed_line_fails_closed(tmp_path: Path):
    module = _load_module()
    source = tmp_path / "crates" / "demo" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn demo() {}\n", encoding="utf-8")
    lcov = _write_lcov(tmp_path / "coverage.lcov", source, {1: 1, 2: 0})

    result = module.audit_changed_coverage(
        tmp_path,
        lcov,
        {
            (source.relative_to(tmp_path).as_posix(), 1),
            (source.relative_to(tmp_path).as_posix(), 2),
        },
        90.0,
    )

    assert result.covered_lines == 1
    assert result.total_lines == 2
    assert any("uncovered changed line" in item for item in result.violations)
    assert any("below required 90.00%" in item for item in result.violations)


def test_changed_production_line_missing_from_lcov_fails_closed(tmp_path: Path):
    module = _load_module()
    source = tmp_path / "crates" / "demo" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn demo() {}\n", encoding="utf-8")
    lcov = _write_lcov(tmp_path / "coverage.lcov", source, {1: 1})

    result = module.audit_changed_coverage(
        tmp_path, lcov, {(source.relative_to(tmp_path).as_posix(), 2)}, 90.0
    )

    assert any("missing from LCOV" in item for item in result.violations)


def test_no_changed_production_lines_is_explicitly_not_a_coverage_claim(tmp_path: Path):
    module = _load_module()
    lcov = tmp_path / "coverage.lcov"
    lcov.write_text("", encoding="utf-8")

    result = module.audit_changed_coverage(tmp_path, lcov, set(), 90.0)

    assert result.violations == []
    assert result.total_lines == 0
