"""Tests for tools/ci/timing/compare_cargo_timings.py.

Covers the BLD-06 compile-regression CI gate diff tool:

1. baseline-vs-current OK case (no regression)
2. threshold-exceeded failure case (both rel and abs exceeded)
3. new-crate-in-current case (no baseline entry)
4. --update-baseline overwrites baseline and exits 0
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "timing" / "compare_cargo_timings.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("compare_cargo_timings", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["compare_cargo_timings"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def _write_json(path: Path, top_repo_crates: list[dict]) -> None:
    payload = {
        "summary": {"profile": "dev", "total_time": "13.2s"},
        "top_units": [],
        "top_repo_crates": top_repo_crates,
    }
    path.write_text(json.dumps(payload), encoding="utf-8")


def _run(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT_PATH), *args],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )


# ---------------------------------------------------------------------------
# load_crates() unit checks
# ---------------------------------------------------------------------------


def test_load_crates_parses_top_repo_crates(tmp_path: Path) -> None:
    p = tmp_path / "summary.json"
    _write_json(
        p,
        [
            {"name": "quanta-index-contract", "duration": 0.99, "units": 1},
            {"name": "quanta-index-lexical", "duration": 0.86, "units": 2},
        ],
    )
    crates = MODULE.load_crates(p)
    assert set(crates) == {"quanta-index-contract", "quanta-index-lexical"}
    assert crates["quanta-index-contract"].duration == 0.99
    assert crates["quanta-index-lexical"].units == 2


def test_load_crates_rejects_missing_top_repo_crates_key(tmp_path: Path) -> None:
    p = tmp_path / "summary.json"
    p.write_text(json.dumps({"summary": {}}), encoding="utf-8")
    import pytest

    with pytest.raises(ValueError, match="missing top_repo_crates"):
        MODULE.load_crates(p)


def test_duplicate_and_empty_timing_rows_are_refused(tmp_path: Path) -> None:
    import pytest

    p = tmp_path / "summary.json"
    _write_json(p, [])
    with pytest.raises(ValueError, match="empty"):
        MODULE.load_crates(p)
    row = {"name": "demo", "duration": 1.0, "units": 1}
    _write_json(p, [row, row])
    with pytest.raises(ValueError, match="duplicate"):
        MODULE.load_crates(p)


def test_ambiguous_timing_json_is_refused(tmp_path: Path) -> None:
    import pytest

    path = tmp_path / "summary.json"
    path.write_text(
        '{"top_repo_crates":[{"name":"demo","duration":1,"units":1}],"top_repo_crates":[]}',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="duplicate JSON key"):
        MODULE.load_crates(path)
    path.write_text(
        '{"top_repo_crates":[{"name":"demo","duration":1,"duration":0,"units":1}]}',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="duplicate JSON key"):
        MODULE.load_crates(path)
    path.write_text(
        '{"top_repo_crates":[{"name":"demo","duration":1,"units":1}],"extra":NaN}',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="non-finite JSON value"):
        MODULE.load_crates(path)


def test_invalid_threshold_and_update_candidate_cannot_pass(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_json(baseline, [{"name": "demo", "duration": 1.0, "units": 1}])
    _write_json(current, [])
    before = baseline.read_bytes()
    update = _run(str(baseline), str(current), "--update-baseline")
    assert update.returncode == 2
    assert baseline.read_bytes() == before
    _write_json(current, [{"name": "demo", "duration": 2.0, "units": 1}])
    threshold = _run(str(baseline), str(current), "--rel-threshold", "nan")
    assert threshold.returncode == 2


# ---------------------------------------------------------------------------
# Case 1: OK — current is within thresholds of baseline
# ---------------------------------------------------------------------------


def test_no_regression_exits_zero(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_json(
        baseline,
        [
            {"name": "quanta-index-contract", "duration": 1.00, "units": 1},
            {"name": "quanta-index-lexical", "duration": 2.00, "units": 1},
        ],
    )
    # tiny growth: +0.05s on lexical (<0.10 abs), no growth on contract.
    _write_json(
        current,
        [
            {"name": "quanta-index-contract", "duration": 1.00, "units": 1},
            {"name": "quanta-index-lexical", "duration": 2.05, "units": 1},
        ],
    )
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "OK: no compile-time regressions" in result.stdout
    assert "REGRESSION" not in result.stdout


def test_small_crate_large_relative_but_tiny_absolute_is_ok(tmp_path: Path) -> None:
    """A +200% relative jump that is < 0.10s absolute must NOT regress."""
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_json(baseline, [{"name": "quanta-index-tiny", "duration": 0.02, "units": 1}])
    _write_json(current, [{"name": "quanta-index-tiny", "duration": 0.06, "units": 1}])
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout
    assert "OK" in result.stdout


# ---------------------------------------------------------------------------
# Case 2: threshold-exceeded failure (both rel >15% AND abs >0.10s)
# ---------------------------------------------------------------------------


def test_regression_when_both_thresholds_exceeded(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_json(
        baseline,
        [{"name": "quanta-index-contract", "duration": 1.00, "units": 1}],
    )
    # +0.50s absolute (>0.10) and +50% relative (>15%) -> REGRESSION
    _write_json(
        current,
        [{"name": "quanta-index-contract", "duration": 1.50, "units": 1}],
    )
    result = _run(str(baseline), str(current))
    assert result.returncode == 1, result.stdout + result.stderr
    assert "REGRESSION" in result.stdout
    assert "quanta-index-contract" in result.stdout
    assert "FAIL" in result.stdout
    assert "--update-baseline" in result.stdout


def test_only_absolute_threshold_exceeded_is_not_regression(tmp_path: Path) -> None:
    """Big absolute jump but tiny relative growth must NOT trigger a regression."""
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    # +0.50s absolute (>0.10) but only +5% relative (<15%) -> OK
    _write_json(baseline, [{"name": "quanta-index-big", "duration": 10.00, "units": 1}])
    _write_json(current, [{"name": "quanta-index-big", "duration": 10.50, "units": 1}])
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout
    assert "REGRESSION" not in result.stdout


def test_custom_thresholds_via_flags(tmp_path: Path) -> None:
    """Passing tighter thresholds should turn a previously-OK delta into a regression."""
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_json(baseline, [{"name": "quanta-index-mid", "duration": 1.00, "units": 1}])
    # +0.20s absolute, +20% relative
    _write_json(current, [{"name": "quanta-index-mid", "duration": 1.20, "units": 1}])

    # Default thresholds (15% / 0.10s): both exceeded -> regression
    default_result = _run(str(baseline), str(current))
    assert default_result.returncode == 1, default_result.stdout

    # Loose thresholds: 25% rel, 0.50s abs -> neither exceeded -> OK
    loose_result = _run(
        str(baseline),
        str(current),
        "--rel-threshold",
        "0.25",
        "--abs-threshold",
        "0.50",
    )
    assert loose_result.returncode == 0, loose_result.stdout


# ---------------------------------------------------------------------------
# Case 3: new crate appears in current but not in baseline
# ---------------------------------------------------------------------------


def test_new_crate_below_abs_threshold_is_ok(tmp_path: Path) -> None:
    """A brand-new crate under 0.10s should not regress (delta <= abs threshold)."""
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_json(baseline, [{"name": "quanta-index-contract", "duration": 1.00, "units": 1}])
    _write_json(
        current,
        [
            {"name": "quanta-index-contract", "duration": 1.00, "units": 1},
            {"name": "quanta-index-newling", "duration": 0.05, "units": 1},
        ],
    )
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout
    # The "new" marker (no baseline) should appear in the table.
    assert "quanta-index-newling" in result.stdout
    assert "new" in result.stdout


def test_new_crate_above_abs_threshold_regresses(tmp_path: Path) -> None:
    """A brand-new crate whose duration exceeds the abs threshold is a regression.

    Per the code: when base_s == 0, the relative-threshold leg is satisfied by
    short-circuit (`base_s == 0 or rel > rel_threshold`), so only the absolute
    delta needs to clear `--abs-threshold`.
    """
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_json(baseline, [{"name": "quanta-index-contract", "duration": 1.00, "units": 1}])
    _write_json(
        current,
        [
            {"name": "quanta-index-contract", "duration": 1.00, "units": 1},
            {"name": "quanta-index-bigling", "duration": 0.75, "units": 1},
        ],
    )
    result = _run(str(baseline), str(current))
    assert result.returncode == 1, result.stdout + result.stderr
    assert "quanta-index-bigling" in result.stdout
    assert "REGRESSION" in result.stdout


def test_crate_removed_in_current_is_invalid_evidence(tmp_path: Path) -> None:
    """A missing baseline crate cannot be interpreted as a timing improvement."""
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_json(
        baseline,
        [
            {"name": "quanta-index-contract", "duration": 1.00, "units": 1},
            {"name": "quanta-index-gone", "duration": 5.00, "units": 1},
        ],
    )
    _write_json(current, [{"name": "quanta-index-contract", "duration": 1.00, "units": 1}])
    result = _run(str(baseline), str(current))
    assert result.returncode == 2, result.stdout + result.stderr
    assert "omits baseline crates" in result.stderr
    assert "quanta-index-gone" in result.stderr
    assert "REGRESSION" not in result.stdout


# ---------------------------------------------------------------------------
# Case 4: --update-baseline overwrites and exits 0
# ---------------------------------------------------------------------------


def test_update_baseline_overwrites_and_exits_zero(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    # Baseline starts small.
    _write_json(baseline, [{"name": "quanta-index-contract", "duration": 1.00, "units": 1}])
    # Current would otherwise blow the regression budget (+1.00s, +100%).
    _write_json(current, [{"name": "quanta-index-contract", "duration": 2.00, "units": 1}])

    # Sanity: without the flag this is a regression.
    fail_result = _run(str(baseline), str(current))
    assert fail_result.returncode == 1, fail_result.stdout

    # With --update-baseline: exits 0 and rewrites baseline to match current verbatim.
    update_result = _run(str(baseline), str(current), "--update-baseline")
    assert update_result.returncode == 0, update_result.stdout + update_result.stderr
    assert "updated baseline" in update_result.stdout
    assert str(baseline) in update_result.stdout

    assert baseline.read_text(encoding="utf-8") == current.read_text(encoding="utf-8")

    # Re-running the diff against the updated baseline must now pass.
    rerun = _run(str(baseline), str(current))
    assert rerun.returncode == 0, rerun.stdout
    assert "OK" in rerun.stdout


def test_update_baseline_creates_baseline_when_missing(tmp_path: Path) -> None:
    """When the baseline path does not yet exist, --update-baseline should create it."""
    baseline = tmp_path / "nonexistent_baseline.json"
    current = tmp_path / "current.json"
    _write_json(current, [{"name": "quanta-index-contract", "duration": 1.00, "units": 1}])
    assert not baseline.exists()
    result = _run(str(baseline), str(current), "--update-baseline")
    assert result.returncode == 0, result.stdout + result.stderr
    assert baseline.exists()
    assert baseline.read_text(encoding="utf-8") == current.read_text(encoding="utf-8")
