"""Contract tests for the fail-closed changed-line coverage gate."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

import pytest

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


def test_duplicate_lcov_line_and_partial_record_are_refused(tmp_path: Path):
    import pytest

    module = _load_module()
    source = tmp_path / "crates" / "demo" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn demo() {}\n", encoding="utf-8")
    lcov = tmp_path / "coverage.lcov"
    lcov.write_text(f"SF:{source}\nDA:1,0\nDA:1,1\nend_of_record\n", encoding="utf-8")
    with pytest.raises(ValueError, match="duplicate LCOV DA"):
        module.parse_lcov(tmp_path, lcov)
    lcov.write_text(f"SF:{source}\nDA:1,1\n", encoding="utf-8")
    with pytest.raises(ValueError, match="lacks end_of_record"):
        module.parse_lcov(tmp_path, lcov)


@pytest.mark.parametrize("filename", ["lib.rs", "한글.rs", "tab\tfile.rs", 'quote"file.rs'])
def test_actual_git_diff_preserves_every_changed_production_path(tmp_path: Path, filename: str):
    module = _load_module()

    def git(*args):
        return subprocess.run(
            ["git", *args], cwd=tmp_path, check=True, capture_output=True, text=True
        ).stdout.strip()

    git("init", "-q")
    git("config", "user.name", "Coverage oracle")
    git("config", "user.email", "coverage@example.invalid")
    git("config", "diff.noprefix", "true")
    path = tmp_path / "crates/demo/src" / filename
    path.parent.mkdir(parents=True)
    path.write_text("pub fn demo() {}\n")
    git("add", ".")
    git("commit", "-qm", "base")
    base = git("rev-parse", "HEAD")
    path.write_text('pub fn demo() { panic!("new uncovered behavior"); }\n')
    git("add", ".")
    git("commit", "-qm", "candidate")
    changed = module.changed_production_lines(tmp_path, base)
    assert changed == {(path.relative_to(tmp_path).as_posix(), 1)}
    lcov = _write_lcov(tmp_path / "coverage.lcov", path, {1: 0})
    verdict = module.audit_changed_coverage(tmp_path, lcov, changed, 90.0)
    assert verdict.total_lines == 1
    assert verdict.violations
