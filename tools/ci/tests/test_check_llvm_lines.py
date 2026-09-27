"""LLVM budgets require one positive total and unambiguous numeric baselines."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "lint/check-llvm-lines.py"
SPEC = importlib.util.spec_from_file_location("check_llvm_lines", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


@pytest.mark.parametrize(
    "body",
    [
        "",
        "30 2 (TOTAL)\n40 3 (TOTAL)\n",
        "30 2 (TOTAL)\ninvalid (TOTAL)\n",
        "30 2 (TOTAL) trailing\n",
        "0 1 (TOTAL)\n",
        "30 0 (TOTAL)\n",
    ],
)
def test_invalid_totals_are_refused(body: str) -> None:
    with pytest.raises(RuntimeError):
        MODULE.parse_total(body)


def test_complete_total_is_accepted() -> None:
    assert (
        MODULE.parse_total("Lines Copies Function\n 30000 500 (TOTAL)\n 50 2 function\n") == 30000
    )


@pytest.mark.parametrize(
    "body",
    ['{"a":1,"a":999999}', '{"a":true}', '{"a":-1}', '{"a":0}', '{"a":1.5}', "{}", "[]"],
)
def test_invalid_baselines_are_refused(monkeypatch, tmp_path: Path, body: str) -> None:
    path = tmp_path / "baseline.json"
    path.write_text(body)
    monkeypatch.setattr(MODULE, "BASELINE_PATH", path)
    with pytest.raises(ValueError):
        MODULE.load_baseline()


def test_missing_baseline_is_refused(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(MODULE, "BASELINE_PATH", tmp_path / "missing.json")
    with pytest.raises(RuntimeError):
        MODULE.load_baseline()


def test_empty_package_selection_is_refused(monkeypatch) -> None:
    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--packages"])
    with pytest.raises(SystemExit) as error:
        MODULE.parse_args()
    assert error.value.code == 2


def test_focused_update_preserves_unmeasured_budget(monkeypatch, tmp_path: Path) -> None:
    path = tmp_path / "baseline.json"
    path.write_text('{"a":100,"b":200}')
    monkeypatch.setattr(MODULE, "BASELINE_PATH", path)
    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--packages", "a", "--update-baseline"])
    monkeypatch.setattr(MODULE, "measure_llvm_lines", lambda package: 120)
    assert MODULE.main() == 0
    assert MODULE.load_baseline() == {"a": 120, "b": 200}
